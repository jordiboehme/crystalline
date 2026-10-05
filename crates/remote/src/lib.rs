//! `crystalline-remote` is the GitHub-backed team collaboration plumbing for
//! Crystalline: a forge-neutral [`Provider`] trait, the plain-text merge
//! engine and the on-disk origin state that let a Domain track a GitHub
//! repository. Every operation goes through the GitHub REST and Git Data
//! APIs behind [`Provider`]; git itself is never invoked as a binary or a
//! library, so it stays an implementation detail hidden from users and from
//! the rest of the workspace.
//!
//! Merge is three-way (base, local, upstream): the frontmatter key by key
//! through [`crystalline_core::frontmatter::merge_text`], the body and any
//! frontmatter that cannot be cut into keys line by line.
//!
//! This crate depends on `crystalline-core` only among workspace crates: it
//! reads and writes plain files (the working tree, the base snapshot, origin
//! state) and never touches the search index directly.
//! `crystalline-service` orchestrates the files this crate produces into the
//! existing sync engine.

pub mod archive;
pub mod changes;
pub mod error;
pub mod github;
pub mod merge;
pub mod mounts;
pub mod ops;
pub mod provider;
pub mod server_token;
pub mod sources;
pub mod state;
pub mod token;

pub use error::RemoteError;
pub use github::GitHubProvider;
pub use github::auth::{DeviceFlowStart, DevicePoll, GITHUB_CLIENT_ID};
pub use github::{validate_repo, validate_repo_path};
pub use mounts::{
    Announcement, LocalDomain, Mount, MountTable, NameMap, OriginIdentity, RemoteDomain, Route,
    Skipped, ToolShape, assign, translate_answer,
};
pub use ops::{
    ChangeSides, CommitReport, DIRECT_NO_AMEND, DiscardRefusal, DiscardReport, DiscardTarget,
    OriginStatusReport, PlannedAction, ProposeOutcome, ProposeReport, PullReport, SharePlan,
    SubscribeReport, discard_local_files, local_change_sides, materialise_base_paths, propose,
    propose_preview, pull, resolve_local_change, status, subscribe, unshared_base,
};
pub use provider::{
    ChangeKind, CompareResult, Feedback, HeadProbe, OpenProposalRef, OriginSpec, ProposalHandle,
    ProposalRequest, ProposalState, Provider, StackInfo, StackMember, TreeWrite, UpstreamChange,
};
pub use server_token::{CredentialKind, ServerCredential, ServerCredentialStore, server_key};
pub use sources::{
    LOCAL_SUFFIX, MountRecord, REMOTE_TOKEN_ENV, REMOTE_URL_ENV, SOURCES_FILE, SourceRecord,
    SourcesFile, default_source_name, env_source, load_sources, remote_dir, update_sources,
    valid_source_name,
};
pub use token::{
    MAX_IDENTITY_NAME_BYTES, StoredToken, TokenIdentity, TokenStore, valid_identity_name,
};
