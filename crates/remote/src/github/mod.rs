//! The GitHub implementation of [`Provider`], built over the GitHub REST and
//! Git Data APIs.
//!
//! Every request carries the standard GitHub REST headers (an explicit API
//! version, the `+json` media type and a `User-Agent`) plus a bearer token
//! when one is configured; the token itself is out of scope here (a later
//! task owns the device flow and where it is stored), so [`GitHubProvider`]
//! just carries whatever `Option<String>` it is handed.
//!
//! **Compare pagination.** GitHub's own documentation for the compare
//! endpoint says the changed-file list is only ever returned on the first
//! page of a paginated comparison, and is capped at 300 files regardless of
//! paging (paging there walks the *commit* list, not the file list). Older
//! API behavior, other GitHub-compatible forges and any future change could
//! still spread files across pages, so [`GitHubProvider::compare`] follows
//! `page` for as long as a page comes back full; in the documented case that
//! costs at most one harmless extra request, and it is the shape covered by
//! the pagination test in `tests/github_client.rs`.

mod types;

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use reqwest::{Method, Response, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::RemoteError;
use crate::provider::{
    ChangeKind, CompareResult, HeadProbe, OriginSpec, ProposalHandle, ProposalRequest,
    ProposalState, Provider, TreeWrite, UpstreamChange,
};
use types::{
    BlobResponse, CommitResponse, CompareFile, CompareResponse, CreateBlobRequest,
    CreateCommitRequest, CreateProposalRequest, CreateProposalResponse, CreateRefRequest,
    CreateTreeRequest, CurrentUserResponse, ErrorBody, ProposalStateResponse, RefResponse,
    ShaResponse, TreeEntryRequest,
};

/// The default GitHub REST API base url.
const DEFAULT_API_URL: &str = "https://api.github.com";

/// The API version pinned in every request's `X-GitHub-Api-Version` header.
const API_VERSION: &str = "2022-11-28";

/// How many changed files [`GitHubProvider::compare`] asks for per page.
const COMPARE_PER_PAGE: usize = 100;

/// The documented cap on how many changed files the compare endpoint reports
/// for one comparison. Reaching it means there may be more files than shown;
/// callers fall back to a tarball diff against the base snapshot.
const COMPARE_FILES_CAP: usize = 300;

/// The per-request timeout. Generous enough for a large tarball download,
/// short enough that a stalled connection is reported as
/// [`RemoteError::Offline`] rather than hanging forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// A [`Provider`] backed by the GitHub REST and Git Data APIs over HTTP.
pub struct GitHubProvider {
    client: reqwest::Client,
    api_url: String,
    token: Option<String>,
}

impl GitHubProvider {
    /// Builds a client against `api_url` (default `https://api.github.com`)
    /// carrying `token` as a bearer credential. `token: None` sends
    /// unauthenticated requests, which works against public repositories.
    pub fn new(api_url: Option<String>, token: Option<String>) -> GitHubProvider {
        let api_url = api_url
            .map(|url| url.trim_end_matches('/').to_string())
            .unwrap_or_else(|| DEFAULT_API_URL.to_string());
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("the client config here is static and always valid");
        GitHubProvider {
            client,
            api_url,
            token,
        }
    }

    /// Starts a request against `path` (which must start with `/`),
    /// attaching the standard GitHub headers and the bearer token when one
    /// is configured.
    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{path}", self.api_url);
        let mut builder = self
            .client
            .request(method, url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("User-Agent", "crystalline");
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        builder
    }

    /// Sends a request, mapping a transport-level failure (no response at
    /// all: DNS failure, connection refused, a timeout) to
    /// [`RemoteError::Offline`].
    async fn send(&self, builder: reqwest::RequestBuilder) -> Result<Response, RemoteError> {
        builder.send().await.map_err(|_| RemoteError::Offline)
    }

    /// Checks a response's status, mapping any non-2xx answer to the
    /// matching [`RemoteError`] variant. `repo` names the repository for
    /// endpoints scoped to one, so a 404 there becomes
    /// [`RemoteError::RepoNotFound`]; endpoints with no single repository in
    /// scope (`current_user`) pass `None` and a 404 falls through to the
    /// generic [`RemoteError::Api`].
    async fn check(&self, response: Response, repo: Option<&str>) -> Result<Response, RemoteError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        if status == StatusCode::UNAUTHORIZED {
            return Err(if self.token.is_some() {
                RemoteError::AuthExpired
            } else {
                RemoteError::NotConnected
            });
        }

        if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
            let remaining = header(&response, "x-ratelimit-remaining");
            let retry_after = header(&response, "retry-after");
            if remaining.as_deref() == Some("0") || retry_after.is_some() {
                let reset_at = header(&response, "x-ratelimit-reset");
                return Err(RemoteError::RateLimited {
                    reset: parse_reset(reset_at.as_deref(), retry_after.as_deref()),
                });
            }
        }

        if status == StatusCode::NOT_FOUND
            && let Some(repo) = repo
        {
            return Err(RemoteError::RepoNotFound {
                repo: repo.to_string(),
            });
        }

        let message = error_message(response).await;
        Err(RemoteError::Api {
            status: status.as_u16(),
            message,
        })
    }
}

#[async_trait]
impl Provider for GitHubProvider {
    async fn branch_head(
        &self,
        origin: &OriginSpec,
        etag: Option<&str>,
    ) -> Result<HeadProbe, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/git/ref/heads/{}", origin.branch);
        let mut builder = self.request(Method::GET, &path);
        if let Some(etag) = etag {
            builder = builder.header("If-None-Match", etag);
        }
        let response = self.send(builder).await?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(HeadProbe::Unchanged);
        }
        let response = self.check(response, Some(&origin.repo)).await?;
        let etag = header(&response, "etag");
        let body: RefResponse = parse_json(response).await?;
        Ok(HeadProbe::Changed {
            head: body.object.sha,
            etag,
        })
    }

    async fn compare(
        &self,
        origin: &OriginSpec,
        base: &str,
        head: &str,
    ) -> Result<CompareResult, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let mut files = Vec::new();
        let mut truncated = false;
        let mut page = 1usize;
        loop {
            let path = format!(
                "/repos/{owner}/{name}/compare/{base}...{head}?per_page={COMPARE_PER_PAGE}&page={page}"
            );
            let response = self.send(self.request(Method::GET, &path)).await?;
            let response = self.check(response, Some(&origin.repo)).await?;
            let body: CompareResponse = parse_json(response).await?;
            let page_files = body.files.unwrap_or_default();
            let page_count = page_files.len();
            for file in page_files {
                files.push(map_compare_file(file));
            }
            if files.len() >= COMPARE_FILES_CAP {
                truncated = true;
                break;
            }
            if page_count < COMPARE_PER_PAGE {
                break;
            }
            page += 1;
        }
        Ok(CompareResult { files, truncated })
    }

    async fn blob(&self, origin: &OriginSpec, sha: &str) -> Result<Vec<u8>, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/git/blobs/{sha}");
        let response = self.send(self.request(Method::GET, &path)).await?;
        let response = self.check(response, Some(&origin.repo)).await?;
        let body: BlobResponse = parse_json(response).await?;
        decode_base64(&body.content)
    }

    async fn tarball(&self, origin: &OriginSpec, commit: &str) -> Result<Vec<u8>, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/tarball/{commit}");
        let response = self.send(self.request(Method::GET, &path)).await?;
        let response = self.check(response, Some(&origin.repo)).await?;
        let status = response.status().as_u16();
        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| RemoteError::Api {
                status,
                message: format!("could not read the tarball response body: {e}"),
            })
    }

    async fn create_blob(
        &self,
        origin: &OriginSpec,
        content: &[u8],
    ) -> Result<String, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/git/blobs");
        let body = CreateBlobRequest {
            content: BASE64.encode(content),
            encoding: "base64",
        };
        let response = self
            .send(self.request(Method::POST, &path).json(&body))
            .await?;
        let response = self.check(response, Some(&origin.repo)).await?;
        let body: ShaResponse = parse_json(response).await?;
        Ok(body.sha)
    }

    async fn create_tree(
        &self,
        origin: &OriginSpec,
        parent_commit: &str,
        writes: &[TreeWrite],
    ) -> Result<String, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;

        let commit_path = format!("/repos/{owner}/{name}/git/commits/{parent_commit}");
        let response = self.send(self.request(Method::GET, &commit_path)).await?;
        let response = self.check(response, Some(&origin.repo)).await?;
        let commit: CommitResponse = parse_json(response).await?;

        let entries = writes
            .iter()
            .map(|write| TreeEntryRequest {
                path: write.path.clone(),
                mode: "100644",
                kind: "blob",
                sha: write.blob_sha.clone(),
            })
            .collect();
        let body = CreateTreeRequest {
            base_tree: commit.tree.sha,
            tree: entries,
        };
        let tree_path = format!("/repos/{owner}/{name}/git/trees");
        let response = self
            .send(self.request(Method::POST, &tree_path).json(&body))
            .await?;
        let response = self.check(response, Some(&origin.repo)).await?;
        let body: ShaResponse = parse_json(response).await?;
        Ok(body.sha)
    }

    async fn create_commit(
        &self,
        origin: &OriginSpec,
        message: &str,
        tree: &str,
        parent: &str,
    ) -> Result<String, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/git/commits");
        let body = CreateCommitRequest {
            message: message.to_string(),
            tree: tree.to_string(),
            parents: vec![parent.to_string()],
        };
        let response = self
            .send(self.request(Method::POST, &path).json(&body))
            .await?;
        let response = self.check(response, Some(&origin.repo)).await?;
        let body: ShaResponse = parse_json(response).await?;
        Ok(body.sha)
    }

    async fn create_branch(
        &self,
        origin: &OriginSpec,
        name: &str,
        commit: &str,
    ) -> Result<(), RemoteError> {
        let (owner, repo_name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{repo_name}/git/refs");
        let body = CreateRefRequest {
            reference: format!("refs/heads/{name}"),
            sha: commit.to_string(),
        };
        let response = self
            .send(self.request(Method::POST, &path).json(&body))
            .await?;
        self.check(response, Some(&origin.repo)).await?;
        Ok(())
    }

    async fn delete_branch(&self, origin: &OriginSpec, name: &str) -> Result<(), RemoteError> {
        let (owner, repo_name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{repo_name}/git/refs/heads/{name}");
        let response = self.send(self.request(Method::DELETE, &path)).await?;
        self.check(response, Some(&origin.repo)).await?;
        Ok(())
    }

    async fn create_proposal(
        &self,
        origin: &OriginSpec,
        req: &ProposalRequest,
    ) -> Result<ProposalHandle, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/pulls");
        let body = CreateProposalRequest {
            title: req.title.clone(),
            body: req.body.clone(),
            head: req.branch.clone(),
            base: req.base_branch.clone(),
        };
        let response = self
            .send(self.request(Method::POST, &path).json(&body))
            .await?;
        let response = self.check(response, Some(&origin.repo)).await?;
        let body: CreateProposalResponse = parse_json(response).await?;
        Ok(ProposalHandle {
            number: body.number,
            url: body.html_url,
        })
    }

    async fn proposal_state(
        &self,
        origin: &OriginSpec,
        number: u64,
    ) -> Result<ProposalState, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/pulls/{number}");
        let response = self.send(self.request(Method::GET, &path)).await?;
        let response = self.check(response, Some(&origin.repo)).await?;
        let body: ProposalStateResponse = parse_json(response).await?;
        let merged = body.merged.unwrap_or(false) || body.merged_at.is_some();
        Ok(if merged {
            ProposalState::Merged
        } else if body.state == "open" {
            ProposalState::Open
        } else {
            ProposalState::Declined
        })
    }

    async fn current_user(&self) -> Result<String, RemoteError> {
        let response = self.send(self.request(Method::GET, "/user")).await?;
        let response = self.check(response, None).await?;
        let body: CurrentUserResponse = parse_json(response).await?;
        Ok(body.login)
    }
}

/// Splits `repo` (`owner/name`) into its two halves. `OriginSpec.repo` is
/// always built this way upstream; this only guards against a malformed
/// value reaching the client rather than validating user input.
fn split_repo(repo: &str) -> Result<(&str, &str), RemoteError> {
    repo.split_once('/').ok_or_else(|| RemoteError::Api {
        status: 0,
        message: format!("'{repo}' is not an owner/name GitHub repository"),
    })
}

/// Reads a response header as a plain string, if present and valid UTF-8.
fn header(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Resolves a rate limit reset time: `x-ratelimit-reset` is a Unix epoch
/// timestamp, `retry-after` is a delta in seconds from now. The former is
/// preferred when both are present.
fn parse_reset(
    ratelimit_reset: Option<&str>,
    retry_after: Option<&str>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Some(epoch) = ratelimit_reset.and_then(|value| value.parse::<i64>().ok()) {
        return chrono::DateTime::from_timestamp(epoch, 0);
    }
    let delta = retry_after.and_then(|value| value.parse::<i64>().ok())?;
    Some(chrono::Utc::now() + chrono::Duration::seconds(delta))
}

/// Reads a non-2xx response body and extracts a human-readable message: the
/// `message` field of GitHub's usual JSON error shape when present, else the
/// raw body trimmed to a reasonable length, else a placeholder.
async fn error_message(response: Response) -> String {
    let text = match response.text().await {
        Ok(text) => text,
        Err(_) => return "no error message provided".to_string(),
    };
    if let Ok(ErrorBody {
        message: Some(message),
    }) = serde_json::from_str::<ErrorBody>(&text)
    {
        return message;
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "no error message provided".to_string()
    } else {
        trimmed.chars().take(500).collect()
    }
}

/// Parses a successful response's JSON body, mapping a malformed body (an
/// answer this client did not expect) to [`RemoteError::Api`].
async fn parse_json<T: DeserializeOwned>(response: Response) -> Result<T, RemoteError> {
    let status = response.status().as_u16();
    response.json::<T>().await.map_err(|e| RemoteError::Api {
        status,
        message: format!("could not parse the response body: {e}"),
    })
}

/// Decodes a base64 blob body as GitHub sends it: wrapped with embedded
/// newlines every so many characters.
fn decode_base64(content: &str) -> Result<Vec<u8>, RemoteError> {
    let stripped: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    BASE64.decode(stripped).map_err(|e| RemoteError::Api {
        status: 0,
        message: format!("GitHub returned content that does not decode as base64: {e}"),
    })
}

/// Maps one compare-endpoint file entry to the forge-neutral [`UpstreamChange`].
/// `blob_sha` is `None` for a removal even though GitHub still reports a
/// `sha` on that entry, matching [`UpstreamChange::blob_sha`]'s documented
/// meaning of "the blob sha of the new content".
fn map_compare_file(file: CompareFile) -> UpstreamChange {
    let removed = file.status == "removed";
    let kind = match file.status.as_str() {
        "added" => ChangeKind::Added,
        "removed" => ChangeKind::Removed,
        "renamed" => ChangeKind::Renamed {
            previous: file.previous_filename.unwrap_or_default(),
        },
        // "modified" is the common case; GitHub also uses "changed" for a
        // handful of situations (for example a mode-only change) that this
        // client treats the same way.
        _ => ChangeKind::Modified,
    };
    UpstreamChange {
        path: file.filename,
        kind,
        blob_sha: if removed { None } else { Some(file.sha) },
    }
}
