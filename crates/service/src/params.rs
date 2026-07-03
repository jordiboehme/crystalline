//! Tool and command parameter structs.
//!
//! These are shared by the MCP tool router (each tool takes one
//! `Parameters<T>`) and the CLI data commands (which build the same structs from
//! flags) and are consumed by the [`crate::engine::Engine`]. They derive
//! `Deserialize` for the wire form and `JsonSchema` so the MCP input schemas are
//! generated from the doc comments here. Required fields have no default;
//! optional fields are `Option` and defaulted in the engine so the defaults are
//! documented in one place.

use schemars::JsonSchema;
use serde::Deserialize;

/// Parameters for `write_engram`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteParams {
    /// The target domain. Required; there is no default domain for writes.
    pub domain: String,
    /// The engram title. Used for the filename slug and for `[[Title]]` linking.
    pub title: String,
    /// The markdown body. Observations `- [category] text` and relations
    /// `- rel_type [[Target]]` in the body are parsed and indexed.
    pub content: String,
    /// A domain-relative subfolder to place the engram in. Defaults to the root.
    #[serde(default)]
    pub folder: Option<String>,
    /// The engram `type`. Defaults to `engram`. Recommended values: engram,
    /// guide, decision, architecture, runbook, reference (guidance, not enforced).
    #[serde(rename = "type", default)]
    pub engram_type: Option<String>,
    /// Tags, lowercase-with-hyphens. At least one is recommended.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Lifecycle `status`. Defaults to `current`. Recommended values: current,
    /// implemented, draft, proposed, idea, poc, deprecated, superseded, archived,
    /// legacy (guidance so an agent can tell an idea apart from current fact).
    #[serde(default)]
    pub status: Option<String>,
    /// Extra frontmatter keys, preserved verbatim and filterable.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Overwrite an existing engram with the same permalink instead of erroring.
    #[serde(default)]
    pub overwrite: bool,
}

/// Parameters for `read_engram`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadParams {
    /// A permalink, `domain/permalink`, title or `crystalline://` URL.
    pub identifier: String,
    /// Restrict resolution to this domain.
    #[serde(default)]
    pub domain: Option<String>,
}

/// Parameters for `edit_engram`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EditParams {
    /// A permalink, `domain/permalink`, title or `crystalline://` URL.
    pub identifier: String,
    /// The engram's domain.
    pub domain: String,
    /// One of append, prepend, find_replace, replace_section,
    /// insert_before_section, insert_after_section.
    pub operation: String,
    /// The content to add or the replacement text.
    pub content: String,
    /// The section heading path for the *_section operations, for example
    /// `## API > ### Auth`. Subsections are kept unless include_subsections.
    #[serde(default)]
    pub section: Option<String>,
    /// The text to find, for find_replace.
    #[serde(default)]
    pub find_text: Option<String>,
    /// The exact number of replacements expected; find_replace errors on a
    /// mismatch instead of editing.
    #[serde(default)]
    pub expected_replacements: Option<usize>,
    /// Replace deeper subsections too when replacing a section.
    #[serde(default)]
    pub include_subsections: bool,
    /// The checksum from a prior `read_engram`, guarding a virtual-domain edit
    /// against a change since it was read: the edit is refused as a conflict if
    /// the stored checksum no longer matches. Omit for last-write-wins. Ignored
    /// by file domains, whose single-writer host owns the file on disk.
    #[serde(default)]
    pub expected_checksum: Option<String>,
}

/// Parameters for `move_engram`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MoveParams {
    /// A permalink, `domain/permalink`, title or `crystalline://` URL.
    pub identifier: String,
    /// The engram's current domain.
    pub domain: String,
    /// The new domain-relative path (with or without the `.md` suffix).
    pub destination: String,
    /// Move to a different domain. Cross-domain moves rewrite inbound bare links
    /// to the domain-prefixed form.
    #[serde(default)]
    pub destination_domain: Option<String>,
    /// Rewrite inbound links on a cross-domain move. Defaults to true.
    #[serde(default)]
    pub update_links: Option<bool>,
}

/// Parameters for `delete_engram`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DeleteParams {
    /// A permalink, `domain/permalink`, title or `crystalline://` URL.
    pub identifier: String,
    /// The engram's domain.
    pub domain: String,
}

/// Parameters for `search_engrams`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// The free-text query. Omit for a filter-only search.
    #[serde(default)]
    pub query: Option<String>,
    /// Restrict to these domains. Defaults to every registered domain.
    #[serde(default)]
    pub domains: Option<Vec<String>>,
    /// Filter by `type`.
    #[serde(rename = "type", default)]
    pub engram_type: Option<String>,
    /// Require all of these tags.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Filter by `status`.
    #[serde(default)]
    pub status: Option<String>,
    /// Frontmatter filters, `{ key: value }` or `{ key: { $gt: n } }`.
    #[serde(default)]
    pub metadata_filters: Option<serde_json::Value>,
    /// Only engrams recorded on or after this ISO date.
    #[serde(default)]
    pub after: Option<String>,
    /// hybrid (default), text, semantic, title or permalink. hybrid and semantic
    /// fall back to text when embeddings are unavailable.
    #[serde(default)]
    pub search_type: Option<String>,
    /// Minimum cosine similarity for a semantic hit.
    #[serde(default)]
    pub min_similarity: Option<f32>,
    /// Page size. Defaults to 10.
    #[serde(default)]
    pub limit: Option<usize>,
    /// One-based page number. Defaults to 1.
    #[serde(default)]
    pub page: Option<usize>,
}

/// Parameters for `build_context`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ContextParams {
    /// A `crystalline://domain/permalink` anchor. A `/*` suffix globs a prefix.
    pub anchor: String,
    /// Traversal depth, 1 to 3. Defaults to 1.
    #[serde(default)]
    pub depth: Option<u8>,
    /// Restrict the returned neighborhood to these domains.
    #[serde(default)]
    pub domains: Option<Vec<String>>,
    /// A recency window such as `7d`; advisory in this version.
    #[serde(default)]
    pub timeframe: Option<String>,
    /// Maximum related engrams beyond the anchors. Defaults to 10.
    #[serde(default)]
    pub max_related: Option<usize>,
}

/// Parameters for `recent_activity`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RecentParams {
    /// Restrict to these domains. Defaults to every registered domain.
    #[serde(default)]
    pub domains: Option<Vec<String>>,
    /// A recency window such as `7d`, `24h` or `2w`. Defaults to `7d`.
    #[serde(default)]
    pub timeframe: Option<String>,
    /// Restrict to these `type` values.
    #[serde(default)]
    pub types: Option<Vec<String>>,
}

/// Parameters for `list_domains`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ListDomainsParams {
    /// Include each domain's MANIFEST `When to Use` routing bullets.
    #[serde(default)]
    pub include_routing: bool,
}

/// Parameters for `browse_domain`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BrowseParams {
    /// The domain to browse.
    pub domain: String,
    /// A domain-relative folder path. Defaults to the root.
    #[serde(default)]
    pub path: Option<String>,
    /// How many folder levels deep to list. Defaults to 1.
    #[serde(default)]
    pub depth: Option<usize>,
    /// An optional glob to filter engram paths.
    #[serde(default)]
    pub glob: Option<String>,
}

/// Parameters for `validate_engrams`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ValidateParams {
    /// The domain whose engrams to validate against its schema engrams.
    pub domain: String,
    /// Validate only this engram.
    #[serde(default)]
    pub identifier: Option<String>,
    /// Validate only engrams of this `type`.
    #[serde(rename = "type", default)]
    pub engram_type: Option<String>,
}

/// Parameters for `infer_schema`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct InferParams {
    /// The domain to infer a schema from.
    pub domain: String,
    /// The `type` whose engrams to generalize into a schema.
    #[serde(rename = "type")]
    pub engram_type: String,
    /// Frequency at or above which a field is suggested. Defaults to 0.25.
    #[serde(default)]
    pub threshold: Option<f64>,
}
