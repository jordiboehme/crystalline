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
//! current", "changed upstream" and "user-edited, leave it" apart.
//!
//! Everything in this module reads the filesystem and hashes bytes; nothing
//! writes outside its own tests. Writing into a harness's config directory
//! is M5's job.

pub mod model;
pub mod receipt;

pub use model::{
    ArtifactFile, DesiredFile, DesiredMcp, DesiredSet, DomainArtifacts, McpArtifact, desired_set,
    harness_supports, is_plain_component, resolve_source_roots, scan_domain,
};
pub use receipt::{
    DomainSources, HarnessState, InstalledFile, InstalledMcp, ProvisionReceipt, SourceStamp, load,
    plain_rel_key, receipt_path, save, sha256_hex,
};
