//! `crystalline-remote` is the GitHub-backed team collaboration plumbing for
//! Crystalline: a forge-neutral [`Provider`] trait, the plain-text merge
//! engine and the on-disk origin state that let a Domain track a GitHub
//! repository. Every operation goes through the GitHub REST and Git Data
//! APIs behind [`Provider`]; git itself is never invoked as a binary or a
//! library, so it stays an implementation detail hidden from users and from
//! the rest of the workspace.
//!
//! Merge is plain-text three-way (base, local, upstream) in v1; a
//! frontmatter-aware merge that reconciles YAML keys structurally rather than
//! by line is future work.
//!
//! This crate depends on `crystalline-core` only among workspace crates: it
//! reads and writes plain files (the working tree, the base snapshot, origin
//! state) and never touches the search index directly.
//! `crystalline-service` orchestrates the files this crate produces into the
//! existing sync engine.

pub mod error;
pub mod github;
pub mod provider;
pub mod token;

pub use error::RemoteError;
pub use github::GitHubProvider;
pub use github::auth::{DeviceFlowStart, DevicePoll, GITHUB_CLIENT_ID};
pub use provider::{
    ChangeKind, CompareResult, HeadProbe, OriginSpec, ProposalHandle, ProposalRequest,
    ProposalState, Provider, TreeWrite, UpstreamChange,
};
pub use token::{StoredToken, TokenStore};
