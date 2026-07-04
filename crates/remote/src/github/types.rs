//! Wire shapes for the GitHub REST and Git Data APIs.
//!
//! Only the fields `GitHubProvider` actually reads or writes are modeled
//! here; every other field GitHub sends back is silently ignored by serde.

use serde::{Deserialize, Serialize};

/// `GET .../git/ref/heads/{branch}` response.
#[derive(Debug, Deserialize)]
pub(super) struct RefResponse {
    pub(super) object: RefObject,
}

#[derive(Debug, Deserialize)]
pub(super) struct RefObject {
    pub(super) sha: String,
}

/// `GET .../compare/{base}...{head}` response. `files` is absent (rather
/// than an empty array) on pages past the first, since the compare endpoint
/// only lists changed files on the first page of a paginated comparison.
#[derive(Debug, Deserialize)]
pub(super) struct CompareResponse {
    #[serde(default)]
    pub(super) files: Option<Vec<CompareFile>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CompareFile {
    pub(super) filename: String,
    pub(super) status: String,
    pub(super) sha: String,
    #[serde(default)]
    pub(super) previous_filename: Option<String>,
}

/// `GET .../git/blobs/{sha}` response.
#[derive(Debug, Deserialize)]
pub(super) struct BlobResponse {
    pub(super) content: String,
}

/// `POST .../git/blobs` request body.
#[derive(Debug, Serialize)]
pub(super) struct CreateBlobRequest {
    pub(super) content: String,
    pub(super) encoding: &'static str,
}

/// Response shape shared by every endpoint that returns just a new sha:
/// creating a blob, a tree or a commit.
#[derive(Debug, Deserialize)]
pub(super) struct ShaResponse {
    pub(super) sha: String,
}

/// `GET .../git/commits/{sha}` response, read only to resolve the base tree
/// for [`super::GitHubProvider::create_tree`].
#[derive(Debug, Deserialize)]
pub(super) struct CommitResponse {
    pub(super) tree: TreeRef,
}

#[derive(Debug, Deserialize)]
pub(super) struct TreeRef {
    pub(super) sha: String,
}

/// `POST .../git/trees` request body.
#[derive(Debug, Serialize)]
pub(super) struct CreateTreeRequest {
    pub(super) base_tree: String,
    pub(super) tree: Vec<TreeEntryRequest>,
}

/// One entry of a tree write. `sha: None` serializes as `"sha": null`, which
/// is how the Git Data API deletes `path` from the tree; omitting the field
/// entirely would leave `path` untouched instead.
#[derive(Debug, Serialize)]
pub(super) struct TreeEntryRequest {
    pub(super) path: String,
    pub(super) mode: &'static str,
    #[serde(rename = "type")]
    pub(super) kind: &'static str,
    pub(super) sha: Option<String>,
}

/// `POST .../git/commits` request body.
#[derive(Debug, Serialize)]
pub(super) struct CreateCommitRequest {
    pub(super) message: String,
    pub(super) tree: String,
    pub(super) parents: Vec<String>,
}

/// `POST .../git/refs` request body.
#[derive(Debug, Serialize)]
pub(super) struct CreateRefRequest {
    #[serde(rename = "ref")]
    pub(super) reference: String,
    pub(super) sha: String,
}

/// `POST .../pulls` request body.
#[derive(Debug, Serialize)]
pub(super) struct CreateProposalRequest {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) head: String,
    pub(super) base: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateProposalResponse {
    pub(super) number: u64,
    pub(super) html_url: String,
}

/// `GET .../pulls/{number}` response, trimmed to the fields that determine
/// [`crate::provider::ProposalState`].
#[derive(Debug, Deserialize)]
pub(super) struct ProposalStateResponse {
    pub(super) state: String,
    #[serde(default)]
    pub(super) merged: Option<bool>,
    #[serde(default)]
    pub(super) merged_at: Option<String>,
}

/// `GET /user` response.
#[derive(Debug, Deserialize)]
pub(super) struct CurrentUserResponse {
    pub(super) login: String,
}

/// The `message` field GitHub includes on most JSON error bodies.
#[derive(Debug, Default, Deserialize)]
pub(super) struct ErrorBody {
    #[serde(default)]
    pub(super) message: Option<String>,
}
