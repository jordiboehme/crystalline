//! Offline mock-server tests for `GitHubProvider`. Every test spins up a
//! throwaway axum server on an ephemeral localhost port and points the client
//! at it with `GitHubProvider::new(Some(base_url), token)`, so the suite never
//! touches the real network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use crystalline_remote::github::GitHubProvider;
use crystalline_remote::{
    ChangeKind, HeadProbe, OriginSpec, ProposalRequest, ProposalState, Provider, RemoteError,
    TreeWrite,
};
use tokio::net::TcpListener;

/// Starts `router` on an ephemeral localhost port and returns its base URL.
/// The server keeps serving for the rest of the test process; each test binds
/// its own port so tests never interfere with one another.
async fn spawn(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

fn origin() -> OriginSpec {
    OriginSpec {
        repo: "acme/brand-knowledge".to_string(),
        subpath: None,
        branch: "main".to_string(),
    }
}

// --- branch_head -------------------------------------------------------------

#[tokio::test]
async fn branch_head_reports_changed_then_unchanged_via_etag() {
    async fn ref_handler(headers: HeaderMap) -> Response {
        if headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some("\"abc123\"") {
            return StatusCode::NOT_MODIFIED.into_response();
        }
        (
            StatusCode::OK,
            [("ETag", "\"abc123\"")],
            axum::Json(serde_json::json!({"object": {"sha": "deadbeefcafe"}})),
        )
            .into_response()
    }

    let app = Router::new().route(
        "/repos/acme/brand-knowledge/git/ref/heads/main",
        get(ref_handler),
    );
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    let probe = provider.branch_head(&origin(), None).await.unwrap();
    match probe {
        HeadProbe::Changed { head, etag } => {
            assert_eq!(head, "deadbeefcafe");
            assert_eq!(etag.as_deref(), Some("\"abc123\""));
        }
        HeadProbe::Unchanged => panic!("expected Changed on the first check"),
    }

    let probe = provider
        .branch_head(&origin(), Some("\"abc123\""))
        .await
        .unwrap();
    assert_eq!(
        probe,
        HeadProbe::Unchanged,
        "a matching If-None-Match answers 304"
    );
}

// --- compare -------------------------------------------------------------

fn added_file(name: &str, sha: &str) -> serde_json::Value {
    serde_json::json!({"filename": name, "status": "added", "sha": sha})
}

#[tokio::test]
async fn compare_stitches_pages_and_maps_every_status() {
    // A first, full page (100 entries: the per_page the client asks for),
    // so the client keeps paging; a second, partial page ends it.
    let mut page1_entries: Vec<serde_json::Value> = (0..99)
        .map(|i| added_file(&format!("file{i}.md"), &format!("sha{i}")))
        .collect();
    page1_entries.push(serde_json::json!({
        "filename": "new-name.md",
        "status": "renamed",
        "previous_filename": "old-name.md",
        "sha": "sha-renamed",
    }));
    assert_eq!(page1_entries.len(), 100, "a full page of 100");

    let page2_entries = vec![
        serde_json::json!({"filename": "modified.md", "status": "modified", "sha": "sha-mod"}),
        serde_json::json!({"filename": "changed.md", "status": "changed", "sha": "sha-changed"}),
        serde_json::json!({"filename": "removed.md", "status": "removed", "sha": "sha-removed"}),
    ];

    let page1 = serde_json::json!({"files": page1_entries});
    let page2 = serde_json::json!({"files": page2_entries});

    let app = Router::new().route(
        "/repos/acme/brand-knowledge/compare/basesha...headsha",
        get(move |Query(params): Query<HashMap<String, String>>| {
            let page1 = page1.clone();
            let page2 = page2.clone();
            async move {
                match params.get("page").map(String::as_str) {
                    Some("2") => Json(page2),
                    _ => Json(page1),
                }
            }
        }),
    );
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    let result = provider
        .compare(&origin(), "basesha", "headsha")
        .await
        .unwrap();

    assert_eq!(result.files.len(), 103, "both pages stitched together");
    assert!(!result.truncated, "103 files is under the documented cap");

    let renamed = result
        .files
        .iter()
        .find(|f| f.path == "new-name.md")
        .expect("the renamed file is present");
    match &renamed.kind {
        ChangeKind::Renamed { previous } => assert_eq!(previous, "old-name.md"),
        other => panic!("expected Renamed, got {other:?}"),
    }
    assert_eq!(renamed.blob_sha.as_deref(), Some("sha-renamed"));

    let removed = result
        .files
        .iter()
        .find(|f| f.path == "removed.md")
        .expect("the removed file is present");
    assert!(matches!(removed.kind, ChangeKind::Removed));
    assert_eq!(removed.blob_sha, None, "a removal carries no blob sha");

    let modified = result
        .files
        .iter()
        .find(|f| f.path == "modified.md")
        .expect("the modified file is present");
    assert!(matches!(modified.kind, ChangeKind::Modified));

    let changed = result
        .files
        .iter()
        .find(|f| f.path == "changed.md")
        .expect("the 'changed' status file is present");
    assert!(
        matches!(changed.kind, ChangeKind::Modified),
        "'changed' maps the same as 'modified'"
    );
}

// --- blob -------------------------------------------------------------

#[tokio::test]
async fn blob_decodes_base64_with_embedded_newlines() {
    // GitHub wraps blob content base64 with embedded newlines; the encoded
    // form of b"hello world" split mid-string, as GitHub emits it.
    let encoded = BASE64.encode(b"hello world");
    let (head, tail) = encoded.split_at(encoded.len() / 2);
    let wrapped = format!("{head}\n{tail}\n");

    let app = Router::new().route(
        "/repos/acme/brand-knowledge/git/blobs/{sha}",
        get(move || {
            let wrapped = wrapped.clone();
            async move { Json(serde_json::json!({"content": wrapped, "encoding": "base64"})) }
        }),
    );
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    let bytes = provider.blob(&origin(), "anysha").await.unwrap();
    assert_eq!(bytes, b"hello world");
}

// --- tarball -------------------------------------------------------------

/// Builds a tiny, deterministic tar.gz archive in memory: one file, one
/// entry, no filesystem access needed.
fn sample_tarball_bytes() -> Vec<u8> {
    let mut archive = Vec::new();
    {
        let encoder = flate2::write::GzEncoder::new(&mut archive, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let data: &[u8] = b"hello from the archive\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, "hello.txt", data).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
    }
    archive
}

#[tokio::test]
async fn tarball_round_trips_bytes_through_a_redirect() {
    let bytes = sample_tarball_bytes();
    let bytes_for_route = bytes.clone();

    let app = Router::new()
        .route(
            "/repos/acme/brand-knowledge/tarball/{commit}",
            get(|| async { axum::response::Redirect::to("/actual-tarball") }),
        )
        .route(
            "/actual-tarball",
            get(move || {
                let bytes = bytes_for_route.clone();
                async move {
                    (
                        StatusCode::OK,
                        [("Content-Type", "application/gzip")],
                        bytes,
                    )
                }
            }),
        );
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    let downloaded = provider.tarball(&origin(), "deadbeef").await.unwrap();
    assert_eq!(
        downloaded, bytes,
        "the redirect is followed and bytes match exactly"
    );
}

// --- create_blob / create_tree / create_commit / create_branch ----------

#[derive(Default)]
struct Captured {
    blob_body: Mutex<Option<serde_json::Value>>,
    tree_body: Mutex<Option<serde_json::Value>>,
    commit_body: Mutex<Option<serde_json::Value>>,
    branch_body: Mutex<Option<serde_json::Value>>,
}

#[tokio::test]
async fn create_blob_tree_commit_and_branch_send_the_documented_bodies() {
    let state = Arc::new(Captured::default());

    let app = Router::new()
        .route(
            "/repos/acme/brand-knowledge/git/blobs",
            post(
                |State(state): State<Arc<Captured>>, Json(body): Json<serde_json::Value>| async move {
                    *state.blob_body.lock().unwrap() = Some(body);
                    Json(serde_json::json!({"sha": "blobsha1"}))
                },
            ),
        )
        .route(
            "/repos/acme/brand-knowledge/git/commits/parentsha1",
            get(|| async { Json(serde_json::json!({"tree": {"sha": "basetree1"}})) }),
        )
        .route(
            "/repos/acme/brand-knowledge/git/trees",
            post(
                |State(state): State<Arc<Captured>>, Json(body): Json<serde_json::Value>| async move {
                    *state.tree_body.lock().unwrap() = Some(body);
                    Json(serde_json::json!({"sha": "treesha1"}))
                },
            ),
        )
        .route(
            "/repos/acme/brand-knowledge/git/commits",
            post(
                |State(state): State<Arc<Captured>>, Json(body): Json<serde_json::Value>| async move {
                    *state.commit_body.lock().unwrap() = Some(body);
                    Json(serde_json::json!({"sha": "commitsha1"}))
                },
            ),
        )
        .route(
            "/repos/acme/brand-knowledge/git/refs",
            post(
                |State(state): State<Arc<Captured>>, Json(body): Json<serde_json::Value>| async move {
                    *state.branch_body.lock().unwrap() = Some(body);
                    (
                        StatusCode::CREATED,
                        Json(serde_json::json!({"ref": "refs/heads/feature-x"})),
                    )
                },
            ),
        )
        .with_state(state.clone());

    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    let content = b"new content";
    let blob_sha = provider.create_blob(&origin(), content).await.unwrap();
    assert_eq!(blob_sha, "blobsha1");

    let tree_sha = provider
        .create_tree(
            &origin(),
            "parentsha1",
            &[
                TreeWrite {
                    path: "a.md".to_string(),
                    blob_sha: Some(blob_sha.clone()),
                },
                TreeWrite {
                    path: "b.md".to_string(),
                    blob_sha: None,
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(tree_sha, "treesha1");

    let commit_sha = provider
        .create_commit(&origin(), "message here", &tree_sha, "parentsha1")
        .await
        .unwrap();
    assert_eq!(commit_sha, "commitsha1");

    provider
        .create_branch(&origin(), "feature-x", &commit_sha)
        .await
        .unwrap();

    let blob_body = state.blob_body.lock().unwrap().clone().unwrap();
    assert_eq!(blob_body["encoding"], "base64");
    assert_eq!(blob_body["content"], BASE64.encode(content));

    let tree_body = state.tree_body.lock().unwrap().clone().unwrap();
    assert_eq!(tree_body["base_tree"], "basetree1");
    assert_eq!(tree_body["tree"][0]["path"], "a.md");
    assert_eq!(tree_body["tree"][0]["mode"], "100644");
    assert_eq!(tree_body["tree"][0]["type"], "blob");
    assert_eq!(tree_body["tree"][0]["sha"], "blobsha1");
    assert_eq!(tree_body["tree"][1]["path"], "b.md");
    assert!(
        tree_body["tree"][1]["sha"].is_null(),
        "deleting a path serializes sha: null rather than omitting it"
    );

    let commit_body = state.commit_body.lock().unwrap().clone().unwrap();
    assert_eq!(commit_body["message"], "message here");
    assert_eq!(commit_body["tree"], "treesha1");
    assert_eq!(commit_body["parents"], serde_json::json!(["parentsha1"]));

    let branch_body = state.branch_body.lock().unwrap().clone().unwrap();
    assert_eq!(branch_body["ref"], "refs/heads/feature-x");
    assert_eq!(branch_body["sha"], "commitsha1");
}

#[tokio::test]
async fn delete_branch_sends_delete_to_the_ref_path() {
    let app = Router::new().route(
        "/repos/acme/brand-knowledge/git/refs/heads/crystalline-share-1",
        delete(|| async { StatusCode::NO_CONTENT }),
    );
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    provider
        .delete_branch(&origin(), "crystalline-share-1")
        .await
        .unwrap();
}

// --- create_proposal / proposal_state -------------------------------------

#[tokio::test]
async fn create_proposal_posts_the_documented_body() {
    let app = Router::new().route(
        "/repos/acme/brand-knowledge/pulls",
        post(|Json(body): Json<serde_json::Value>| async move {
            assert_eq!(body["title"], "Share updates");
            assert_eq!(body["body"], "See what changed.");
            assert_eq!(body["head"], "crystalline/share-1");
            assert_eq!(body["base"], "main");
            Json(serde_json::json!({
                "number": 42,
                "html_url": "https://github.com/acme/brand-knowledge/pull/42",
            }))
        }),
    );
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    let handle = provider
        .create_proposal(
            &origin(),
            &ProposalRequest {
                title: "Share updates".to_string(),
                body: "See what changed.".to_string(),
                branch: "crystalline/share-1".to_string(),
                base_branch: "main".to_string(),
            },
        )
        .await
        .unwrap();

    assert_eq!(handle.number, 42);
    assert_eq!(
        handle.url,
        "https://github.com/acme/brand-knowledge/pull/42"
    );
}

#[tokio::test]
async fn proposal_state_maps_open_merged_and_declined() {
    async fn handler(Path(number): Path<u64>) -> Json<serde_json::Value> {
        let body = match number {
            1 => serde_json::json!({"state": "open", "merged": false}),
            2 => serde_json::json!({
                "state": "closed",
                "merged": true,
                "merged_at": "2026-01-01T00:00:00Z",
            }),
            3 => serde_json::json!({"state": "closed", "merged": false}),
            other => panic!("unexpected pull number {other}"),
        };
        Json(body)
    }
    let app = Router::new().route("/repos/acme/brand-knowledge/pulls/{number}", get(handler));
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    assert_eq!(
        provider.proposal_state(&origin(), 1).await.unwrap(),
        ProposalState::Open
    );
    assert_eq!(
        provider.proposal_state(&origin(), 2).await.unwrap(),
        ProposalState::Merged
    );
    assert_eq!(
        provider.proposal_state(&origin(), 3).await.unwrap(),
        ProposalState::Declined
    );
}

// --- error mapping -------------------------------------------------------

#[tokio::test]
async fn unauthorized_maps_to_auth_expired_with_a_token_and_not_connected_without() {
    async fn handler() -> (StatusCode, Json<serde_json::Value>) {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"message": "Bad credentials"})),
        )
    }
    let app = Router::new().route("/user", get(handler));
    let base = spawn(app).await;

    let with_token = GitHubProvider::new(Some(base.clone()), Some("tok".to_string()));
    let err = with_token.current_user().await.unwrap_err();
    assert!(matches!(err, RemoteError::AuthExpired), "{err:?}");

    let without_token = GitHubProvider::new(Some(base), None);
    let err = without_token.current_user().await.unwrap_err();
    assert!(matches!(err, RemoteError::NotConnected), "{err:?}");
}

#[tokio::test]
async fn rate_limit_headers_map_to_rate_limited_with_a_parsed_reset() {
    async fn handler() -> Response {
        (
            StatusCode::FORBIDDEN,
            [
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "1700000000"),
            ],
            Json(serde_json::json!({"message": "rate limit exceeded"})),
        )
            .into_response()
    }
    let app = Router::new().route(
        "/repos/acme/brand-knowledge/git/ref/heads/main",
        get(handler),
    );
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    let err = provider.branch_head(&origin(), None).await.unwrap_err();
    match err {
        RemoteError::RateLimited { reset } => {
            assert_eq!(reset, chrono::DateTime::from_timestamp(1_700_000_000, 0));
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn retry_after_alone_still_maps_to_rate_limited() {
    async fn handler() -> Response {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "30")],
            Json(serde_json::json!({"message": "slow down"})),
        )
            .into_response()
    }
    let app = Router::new().route(
        "/repos/acme/brand-knowledge/git/ref/heads/main",
        get(handler),
    );
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    let before = chrono::Utc::now();
    let err = provider.branch_head(&origin(), None).await.unwrap_err();
    match err {
        RemoteError::RateLimited { reset: Some(reset) } => {
            assert!(
                reset >= before,
                "the reset is computed from now + retry-after"
            );
        }
        other => panic!("expected a RateLimited with a reset, got {other:?}"),
    }
}

#[tokio::test]
async fn not_found_on_a_repo_scoped_endpoint_maps_to_repo_not_found() {
    async fn handler() -> (StatusCode, Json<serde_json::Value>) {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"message": "Not Found"})),
        )
    }
    let app = Router::new().route(
        "/repos/acme/brand-knowledge/git/ref/heads/main",
        get(handler),
    );
    let base = spawn(app).await;
    let provider = GitHubProvider::new(Some(base), None);

    let err = provider.branch_head(&origin(), None).await.unwrap_err();
    match err {
        RemoteError::RepoNotFound { repo } => assert_eq!(repo, "acme/brand-knowledge"),
        other => panic!("expected RepoNotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn connection_refused_maps_to_offline() {
    // Bind to grab a free port, then drop the listener: nothing answers
    // there, so a connection attempt is refused immediately.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let provider = GitHubProvider::new(Some(format!("http://{addr}")), None);
    let err = provider.current_user().await.unwrap_err();
    assert!(matches!(err, RemoteError::Offline), "{err:?}");
}

// --- auth header presence -------------------------------------------------

#[tokio::test]
async fn authorization_header_is_sent_only_when_a_token_is_configured() {
    async fn handler(headers: HeaderMap) -> Json<serde_json::Value> {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        Json(serde_json::json!({"login": auth}))
    }
    let app = Router::new().route("/user", get(handler));
    let base = spawn(app).await;

    let with_token = GitHubProvider::new(Some(base.clone()), Some("secret-token".to_string()));
    let login = with_token.current_user().await.unwrap();
    assert_eq!(login, "Bearer secret-token");

    let without_token = GitHubProvider::new(Some(base), None);
    let login = without_token.current_user().await.unwrap();
    assert_eq!(
        login, "",
        "no Authorization header reaches the server at all"
    );
}
