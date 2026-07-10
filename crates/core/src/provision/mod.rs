//! Provisioning: turning a domain's declared artifact folders into installed
//! skills, commands, agents and MCP configs inside an AI harness's own
//! config directory.
//!
//! `model` reads a domain's `MANIFEST.md` `## Provisioning` section into
//! resolved source roots, scans those roots into a hashed [`model::DomainArtifacts`]
//! set, and projects every domain's artifacts through one harness's support
//! matrix into a [`model::DesiredSet`] - the keys a reconcile engine (M5) will
//! diff against a harness's live directory. `receipt` is the on-disk memory
//! of what a previous reconcile installed, so a later run can tell "still
//! current", "changed upstream" and "user-edited, leave it" apart. `reconcile`
//! is the engine that finally acts on that diff: writing, updating, adopting
//! and retiring files inside a harness's config directory and registering MCP
//! servers through a runner trait.
//!
//! `model` and `receipt` only ever read the filesystem and hash bytes;
//! `reconcile` is the one place that writes into a harness's config directory.
//! Even there, process work (registering an MCP server through a harness CLI)
//! stays behind the [`reconcile::McpRunner`] trait, so this crate keeps its
//! promise never to spawn a child or depend on an async runtime.

pub mod model;
pub mod receipt;
pub mod reconcile;

pub use model::{
    ArtifactFile, DesiredFile, DesiredMcp, DesiredSet, DomainArtifacts, McpArtifact, desired_set,
    harness_supports, is_plain_component, resolve_source_roots, scan_domain,
};
pub use receipt::{
    DomainSources, HarnessState, InstalledFile, InstalledMcp, ProvisionReceipt, SourceStamp, load,
    plain_rel_key, receipt_path, save, sha256_hex,
};
pub use reconcile::{ActionStatus, ArtifactAction, McpOutcome, McpRunner, reconcile_harness};
