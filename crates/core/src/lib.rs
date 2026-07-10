//! `crystalline-core` holds the parts of Crystalline that must stay static:
//! the Engram markdown format, permalink and address logic, the Manifest
//! model, Picoschema and configuration types. This crate intentionally has
//! no async, database or ML dependencies, so `crystalline verify` and
//! `crystalline prompt system` can run without a service, a socket or a
//! network connection.
//!
//! The format is Google OKF v0.1 (markdown plus YAML frontmatter, only
//! `type` required, unknown keys preserved) extended with Crystalline
//! observations, relations, cross-domain wikilinks and temporal metadata.

/// The crate version, re-exported from the value Cargo embeds at build
/// time so callers do not need to depend on `env!` directly.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod address;
pub mod config;
pub mod emit;
pub mod engram;
pub mod harness;
pub mod import;
pub mod manifest;
pub mod parse;
pub mod prompt;
pub mod provision;
pub mod schema;
pub mod verify;
pub mod yaml;

pub use address::{CrystallineUrl, LinkResolver, LookupTable, Resolution, ResolvedRef, slugify};
pub use emit::emit_engram;
pub use engram::{
    Engram, Frontmatter, Heading, LinkTarget, Observation, RECOMMENDED_STATUSES, RECOMMENDED_TYPES,
    Relation, SchemaDef, WikiLink,
};
pub use harness::{HarnessKind, HarnessPaths, harness_paths};
pub use manifest::{
    ArtifactType, Manifest, ProblemKind, ProvisioningDecl, ProvisioningProblem,
    ProvisioningSection, in_root_artifact_dirs, manifest_template,
};
pub use parse::{LosslessEngram, ParseError, parse_engram, parse_engram_lossless};
pub use prompt::{
    PromptDomain, PromptOutput, generate_prompt, generate_prompt_unscoped, render_instructions,
    render_json, render_text,
};
pub use provision::{
    ArtifactFile, DesiredFile, DesiredMcp, DesiredSet, DomainArtifacts, DomainSources,
    HarnessState, InstalledFile, InstalledMcp, McpArtifact, ProvisionReceipt, SourceStamp,
    desired_set, harness_supports, is_plain_component, resolve_source_roots, scan_domain,
};
pub use schema::{
    FieldDecl, FieldType, ScalarType, Schema, SchemaDrift, SchemaIssue, ValidationMode,
};
pub use verify::{Issue, Severity, VerifyOptions, VerifyReport, verify_paths};
pub use yaml::YamlValue;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn recommended_sets_are_populated() {
        assert!(RECOMMENDED_TYPES.contains(&"engram"));
        assert!(RECOMMENDED_STATUSES.contains(&"current"));
    }
}
