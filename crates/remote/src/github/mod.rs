//! The GitHub implementation of [`Provider`], built over the GitHub REST and
//! Git Data APIs.
//!
//! Every request carries the standard GitHub REST headers (an explicit API
//! version, the `+json` media type and a `User-Agent`) plus a bearer token
//! when one is configured; [`GitHubProvider`] just carries whatever
//! `Option<String>` it is handed, and stays agnostic of where that token
//! came from. [`auth`] owns getting one (the OAuth device flow, or
//! validating a pasted-in personal access token) and [`crate::token`] owns
//! keeping it between runs.
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

pub mod auth;
mod types;

use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use reqwest::{Method, Response, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::RemoteError;
use crate::provider::{
    ChangeKind, CompareResult, Feedback, HeadProbe, OpenProposalRef, OriginSpec, ProposalHandle,
    ProposalRequest, ProposalState, Provider, StackInfo, StackMember, TreeWrite, UpstreamChange,
};
use types::{
    BlobResponse, CloseProposalRequest, CommitResponse, CompareFile, CompareResponse,
    CreateBlobRequest, CreateCommitRequest, CreateProposalRequest, CreateProposalResponse,
    CreateRefRequest, CreateTreeRequest, CurrentUserResponse, ErrorBody, IssueCommentResponse,
    OpenProposalListItem, ProposalStateResponse, RefResponse, ReviewCommentResponse,
    ReviewResponse, ShaResponse, StackResponse, StackWriteRequest, TreeEntryRequest,
    UpdateProposalRequest, UpdateRefRequest,
};

/// The default GitHub REST API base url.
const DEFAULT_API_URL: &str = "https://api.github.com";

/// The API version pinned in every request's `X-GitHub-Api-Version` header.
const API_VERSION: &str = "2022-11-28";

/// How many entries this client asks for per page on every paginated list it
/// walks: the compare endpoint's changed files, the open-proposal list and
/// each of the three feedback channels. 100 is GitHub's documented maximum.
const COMPARE_PER_PAGE: usize = 100;

/// The documented cap on how many changed files the compare endpoint reports
/// for one comparison. Reaching it means there may be more files than shown;
/// callers fall back to a tarball diff against the base snapshot.
const COMPARE_FILES_CAP: usize = 300;

/// How many attempts a stack write makes in total. GitHub answers 409 while
/// the stack is being modified elsewhere, which is a race rather than a
/// refusal, so the write is retried a bounded number of times before the
/// conflict is handed to the caller.
const STACK_WRITE_ATTEMPTS: usize = 3;

/// How long to wait before each retry of a conflicted stack write, one entry
/// per retry, so `STACK_WRITE_ATTEMPTS` attempts wait at most 750ms in total.
const STACK_WRITE_BACKOFF: [Duration; STACK_WRITE_ATTEMPTS - 1] =
    [Duration::from_millis(250), Duration::from_millis(500)];

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

    /// Walks every page of a list endpoint that takes no query parameters of
    /// its own, following `page` for as long as a page comes back full.
    ///
    /// The three feedback channels need this rather than a single capped
    /// request: GitHub returns reviews and comments oldest first, so a
    /// proposal with a long thread would drop exactly the newest feedback,
    /// which is the part the consumer keeps.
    ///
    /// There is deliberately no page ceiling, and that is a trust assumption
    /// rather than an oversight: GitHub terminates a listing with a short
    /// page, so the loop ends on real data. A hostile or broken forge that
    /// answered every page full would keep this walking. The exposure is
    /// bounded by who can be an origin - a repository the user pointed this
    /// machine at - and a cap would trade an honest listing for a silently
    /// truncated one on the very repositories that legitimately have long
    /// review threads, which is the failure this function exists to avoid.
    async fn paged_list<T: DeserializeOwned>(
        &self,
        repo: &str,
        path: &str,
    ) -> Result<Vec<T>, RemoteError> {
        let mut out = Vec::new();
        let mut page = 1usize;
        loop {
            let url = format!("{path}?per_page={COMPARE_PER_PAGE}&page={page}");
            let response = self.send(self.request(Method::GET, &url)).await?;
            let response = self.check(response, Some(repo)).await?;
            let items: Vec<T> = parse_json(response).await?;
            let count = items.len();
            out.extend(items);
            if count < COMPARE_PER_PAGE {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    /// Posts a stack write (creating a stack, or adding layers to one) and
    /// reads the stack it answers with.
    ///
    /// Two answers are read before [`check`](GitHubProvider::check) sees
    /// them. A 404 is the forge saying it has no stack endpoints at all
    /// rather than a missing repository, so it becomes
    /// [`RemoteError::StacksUnsupported`]. A 409 is GitHub reporting that the
    /// stack is being modified concurrently: the write is retried up to
    /// [`STACK_WRITE_ATTEMPTS`] times with [`STACK_WRITE_BACKOFF`] between
    /// the tries, and only a conflict that survives all of them is surfaced,
    /// as an ordinary [`RemoteError::Api`] carrying GitHub's message.
    async fn stack_write(
        &self,
        repo: &str,
        path: &str,
        pull_requests: &[u64],
    ) -> Result<StackInfo, RemoteError> {
        let body = StackWriteRequest {
            pull_requests: pull_requests.to_vec(),
        };
        let mut attempt = 1usize;
        loop {
            let response = self
                .send(self.request(Method::POST, path).json(&body))
                .await?;
            if response.status() == StatusCode::NOT_FOUND {
                return Err(RemoteError::StacksUnsupported);
            }
            if response.status() == StatusCode::CONFLICT && attempt < STACK_WRITE_ATTEMPTS {
                tokio::time::sleep(STACK_WRITE_BACKOFF[attempt - 1]).await;
                attempt += 1;
                continue;
            }
            let response = self.check(response, Some(repo)).await?;
            let stack: StackResponse = parse_json(response).await?;
            return Ok(map_stack(stack));
        }
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
        parents: &[String],
    ) -> Result<String, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/git/commits");
        let body = CreateCommitRequest {
            message: message.to_string(),
            tree: tree.to_string(),
            parents: parents.to_vec(),
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

    async fn branch_ref(
        &self,
        origin: &OriginSpec,
        name: &str,
    ) -> Result<Option<String>, RemoteError> {
        let (owner, repo_name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{repo_name}/git/ref/heads/{name}");
        let response = self.send(self.request(Method::GET, &path)).await?;
        if response.status() == StatusCode::NOT_FOUND {
            // A missing share branch is an answer, not a repository problem:
            // check() would map this 404 to RepoNotFound, so it is read here.
            return Ok(None);
        }
        let response = self.check(response, Some(&origin.repo)).await?;
        let body: RefResponse = parse_json(response).await?;
        Ok(Some(body.object.sha))
    }

    async fn update_branch(
        &self,
        origin: &OriginSpec,
        name: &str,
        commit: &str,
        force: bool,
    ) -> Result<(), RemoteError> {
        let (owner, repo_name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{repo_name}/git/refs/heads/{name}");
        let body = UpdateRefRequest {
            sha: commit.to_string(),
            force,
        };
        let response = self
            .send(self.request(Method::PATCH, &path).json(&body))
            .await?;
        self.check(response, Some(&origin.repo)).await?;
        Ok(())
    }

    async fn update_proposal(
        &self,
        origin: &OriginSpec,
        number: u64,
        title: Option<&str>,
        body: Option<&str>,
        base: Option<&str>,
    ) -> Result<(), RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/pulls/{number}");
        let body = UpdateProposalRequest {
            title: title.map(str::to_string),
            body: body.map(str::to_string),
            base: base.map(str::to_string),
        };
        let response = self
            .send(self.request(Method::PATCH, &path).json(&body))
            .await?;
        self.check(response, Some(&origin.repo)).await?;
        Ok(())
    }

    async fn close_proposal(&self, origin: &OriginSpec, number: u64) -> Result<(), RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/pulls/{number}");
        let body = CloseProposalRequest { state: "closed" };
        let response = self
            .send(self.request(Method::PATCH, &path).json(&body))
            .await?;
        self.check(response, Some(&origin.repo)).await?;
        Ok(())
    }

    async fn proposal_feedback(
        &self,
        origin: &OriginSpec,
        number: u64,
    ) -> Result<Feedback, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;

        let reviews: Vec<ReviewResponse> = self
            .paged_list(
                &origin.repo,
                &format!("/repos/{owner}/{name}/pulls/{number}/reviews"),
            )
            .await?;
        let review_comments: Vec<ReviewCommentResponse> = self
            .paged_list(
                &origin.repo,
                &format!("/repos/{owner}/{name}/pulls/{number}/comments"),
            )
            .await?;
        let issue_comments: Vec<IssueCommentResponse> = self
            .paged_list(
                &origin.repo,
                &format!("/repos/{owner}/{name}/issues/{number}/comments"),
            )
            .await?;

        Ok(build_feedback(reviews, review_comments, issue_comments))
    }

    async fn list_open_proposals(
        &self,
        origin: &OriginSpec,
    ) -> Result<Vec<OpenProposalRef>, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let mut out = Vec::new();
        let mut page = 1usize;
        loop {
            let path = format!(
                "/repos/{owner}/{name}/pulls?state=open&per_page={COMPARE_PER_PAGE}&page={page}"
            );
            let response = self.send(self.request(Method::GET, &path)).await?;
            let response = self.check(response, Some(&origin.repo)).await?;
            let items: Vec<OpenProposalListItem> = parse_json(response).await?;
            let count = items.len();
            for item in items {
                out.push(OpenProposalRef {
                    number: item.number,
                    branch: item.head.reference,
                    head_sha: item.head.sha,
                });
            }
            if count < COMPARE_PER_PAGE {
                break;
            }
            page += 1;
        }
        Ok(out)
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

    async fn list_stacks(
        &self,
        origin: &OriginSpec,
        pull_request: Option<u64>,
    ) -> Result<Vec<StackInfo>, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = match pull_request {
            Some(number) => format!("/repos/{owner}/{name}/stacks?pull_request={number}"),
            None => format!("/repos/{owner}/{name}/stacks"),
        };
        let response = self.send(self.request(Method::GET, &path)).await?;
        if response.status() == StatusCode::NOT_FOUND {
            // A forge that does not serve stacks answers 404 on this path:
            // that is an answer about the forge, not a repository problem,
            // so it is read here before check() maps it to RepoNotFound.
            return Err(RemoteError::StacksUnsupported);
        }
        let response = self.check(response, Some(&origin.repo)).await?;
        let stacks: Vec<StackResponse> = parse_json(response).await?;
        Ok(stacks.into_iter().map(map_stack).collect())
    }

    async fn create_stack(
        &self,
        origin: &OriginSpec,
        pull_requests: &[u64],
    ) -> Result<StackInfo, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/stacks");
        self.stack_write(&origin.repo, &path, pull_requests).await
    }

    async fn extend_stack(
        &self,
        origin: &OriginSpec,
        stack_number: u64,
        pull_requests: &[u64],
    ) -> Result<StackInfo, RemoteError> {
        let (owner, name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{name}/stacks/{stack_number}/add");
        self.stack_write(&origin.repo, &path, pull_requests).await
    }

    async fn dissolve_stack(
        &self,
        origin: &OriginSpec,
        stack_number: u64,
    ) -> Result<(), RemoteError> {
        let (owner, repo_name) = split_repo(&origin.repo)?;
        let path = format!("/repos/{owner}/{repo_name}/stacks/{stack_number}/unstack");
        let response = self.send(self.request(Method::POST, &path)).await?;
        if response.status() == StatusCode::NOT_FOUND {
            // Same reading as list_stacks: no stack endpoints, not a missing
            // repository.
            return Err(RemoteError::StacksUnsupported);
        }
        // GitHub answers 204 when every member left the stack and 200 with
        // whatever remains when some member could not be released (a queued
        // merge holds it). Both mean the request was carried out, and this
        // client has nothing to do with the remainder, so the body is
        // deliberately not read.
        self.check(response, Some(&origin.repo)).await?;
        Ok(())
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

/// Maps one stack, as GitHub reports it, to the forge-neutral [`StackInfo`].
/// Member order is GitHub's own: bottom layer first, which is the order every
/// caller here relies on.
fn map_stack(stack: StackResponse) -> StackInfo {
    StackInfo {
        number: stack.number,
        open: stack.open,
        members: stack
            .pull_requests
            .into_iter()
            .map(|member| StackMember {
                number: member.number,
                state: member.state,
                head_sha: member.head.sha,
            })
            .collect(),
    }
}

/// Folds the three feedback channels into the forge-neutral [`Feedback`].
///
/// Review state: the last reported non-empty state per reviewer wins, "last"
/// meaning last in `reviews` rather than by any timestamp - GitHub returns
/// reviews in chronological order, and this trusts that array order rather
/// than re-sorting on `submitted_at`, which is optional on the wire. A bare
/// COMMENTED event with no body does not displace an earlier verdict, then
/// any reviewer at CHANGES_REQUESTED makes the whole state
/// "changes_requested", else any APPROVED makes it "approved", else any
/// COMMENTED makes it "commented". Review bodies become items only when
/// non-empty; comments by the connected user are kept - they are part of the
/// thread, and filtering them would hide half the conversation.
fn build_feedback(
    reviews: Vec<ReviewResponse>,
    review_comments: Vec<ReviewCommentResponse>,
    issue_comments: Vec<IssueCommentResponse>,
) -> Feedback {
    use std::collections::BTreeMap;
    let mut latest: BTreeMap<String, String> = BTreeMap::new();
    let mut items = Vec::new();
    for review in &reviews {
        let login = review
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_default();
        let meaningful = matches!(
            review.state.as_str(),
            "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED"
        );
        let body_empty = review.body.as_deref().unwrap_or("").trim().is_empty();
        if meaningful && !(review.state == "COMMENTED" && body_empty) {
            latest.insert(login.clone(), review.state.clone());
        }
        if !body_empty {
            items.push(crate::state::FeedbackItem {
                author: login,
                body: review.body.clone().unwrap_or_default(),
                path: None,
                line: None,
                submitted_at: review.submitted_at.clone().unwrap_or_default(),
                kind: crate::state::FeedbackKind::Review,
            });
        }
    }
    let review_state = if latest.values().any(|s| s == "CHANGES_REQUESTED") {
        Some("changes_requested".to_string())
    } else if latest.values().any(|s| s == "APPROVED") {
        Some("approved".to_string())
    } else if latest.values().any(|s| s == "COMMENTED") {
        Some("commented".to_string())
    } else {
        None
    };
    for comment in review_comments {
        items.push(crate::state::FeedbackItem {
            author: comment.user.map(|u| u.login).unwrap_or_default(),
            body: comment.body,
            path: comment.path,
            line: comment.line,
            submitted_at: comment.created_at.unwrap_or_default(),
            kind: crate::state::FeedbackKind::ReviewComment,
        });
    }
    for comment in issue_comments {
        items.push(crate::state::FeedbackItem {
            author: comment.user.map(|u| u.login).unwrap_or_default(),
            body: comment.body,
            path: None,
            line: None,
            submitted_at: comment.created_at.unwrap_or_default(),
            kind: crate::state::FeedbackKind::Comment,
        });
    }
    Feedback {
        review_state,
        items,
    }
}
