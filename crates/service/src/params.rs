//! Tool and command parameter structs.
//!
//! These are shared by the MCP tool router (each tool takes one
//! `Parameters<T>`) and the CLI data commands (which build the same structs from
//! flags) and are consumed by the [`crate::engine::Engine`]. They derive
//! `Deserialize` for the wire form and `JsonSchema` so the MCP input schemas are
//! generated from the doc comments here. Required fields have no default;
//! optional fields are `Option` and defaulted in the engine so the defaults are
//! documented in one place.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;

/// Deserialize a field that may be missing, `null` or a real value, mapping
/// both an absent key and an explicit `null` to `T::default()`. Paired with
/// `#[serde(default)]` (which covers the missing key) so a bare `Vec`/`BTreeMap`
/// param still tolerates the `null` that some clients and the CLI send for an
/// empty list, while the generated schema stays a plain `array`/`object` rather
/// than a `["array", "null"]` union.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

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
    #[serde(default, deserialize_with = "null_as_default")]
    pub tags: Vec<String>,
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
    /// A bare permalink, title or `crystalline://` URL. Without the scheme
    /// the identifier is domain-relative: never prefix it with a domain name.
    pub identifier: String,
    /// Restrict resolution to this domain.
    #[serde(default)]
    pub domain: Option<String>,
}

/// Parameters for `edit_engram`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EditParams {
    /// A bare permalink, title or `crystalline://` URL. Without the scheme
    /// the identifier is domain-relative: never prefix it with a domain name.
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
    /// A bare permalink, title or `crystalline://` URL. Without the scheme
    /// the identifier is domain-relative: never prefix it with a domain name.
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
    /// A bare permalink, title or `crystalline://` URL. Without the scheme
    /// the identifier is domain-relative: never prefix it with a domain name.
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
    #[serde(default, deserialize_with = "null_as_default")]
    pub domains: Vec<String>,
    /// Filter by `type`.
    #[serde(rename = "type", default)]
    pub engram_type: Option<String>,
    /// Require all of these tags.
    #[serde(default, deserialize_with = "null_as_default")]
    pub tags: Vec<String>,
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
    #[serde(default, deserialize_with = "null_as_default")]
    pub domains: Vec<String>,
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
    #[serde(default, deserialize_with = "null_as_default")]
    pub domains: Vec<String>,
    /// A recency window such as `7d`, `24h` or `2w`. Defaults to `7d`.
    #[serde(default)]
    pub timeframe: Option<String>,
    /// Restrict to these `type` values.
    #[serde(default, deserialize_with = "null_as_default")]
    pub types: Vec<String>,
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

/// Parameters for `configure`. Omit everything to see the current settings
/// and GitHub connection. `token` or `connect` handle a GitHub connect
/// action on their own and ignore `set`/`unset` in the same call; give them
/// on a separate call from a settings change.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ConfigureParams {
    /// Settings to change, key to value, for example { "github.enabled":
    /// "true" }. Applied in ascending key order; the first invalid key or
    /// value stops the rest and reports what was already applied. Omit or
    /// pass an empty object to leave settings unchanged.
    #[serde(default, deserialize_with = "null_as_default")]
    pub set: BTreeMap<String, String>,
    /// Setting keys to reset to their default, applied after `set`. Omit or
    /// pass an empty array to leave settings unchanged.
    #[serde(default, deserialize_with = "null_as_default")]
    pub unset: Vec<String>,
    /// Pass "github" to link a GitHub account: starts a short code to
    /// confirm at github.com/login/device, or reports an already-pending
    /// one. Omit when `token` is supplied. Works whether or not
    /// github.enabled is on yet; enabling is only needed for team domains.
    #[serde(default)]
    pub connect: Option<String>,
    /// A GitHub personal access token, connecting immediately instead of the
    /// short-code flow.
    #[serde(default)]
    pub token: Option<String>,
    /// A GitHub Enterprise Server host for this connect only, for example
    /// github.example.com. Durable GitHub Enterprise Server setup is `set
    /// github.api_url`.
    #[serde(default)]
    pub host: Option<String>,
}

/// Parameters for `add_domain`. The mode follows the parameters: `repo` makes
/// it a GitHub team domain, `virtual: true` a database-backed domain, and
/// otherwise it is a local folder domain. `repo` and `virtual` are mutually
/// exclusive.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AddDomainParams {
    /// The domain name. Optional for a team domain (defaults to the
    /// repository's own name) and for a local domain given a `folder` (defaults
    /// to the folder's name); required for a virtual domain.
    #[serde(default)]
    pub domain: Option<String>,
    /// A local folder domain, engrams as markdown files on disk. Where the
    /// domain lives on this machine; created and given a starter `MANIFEST.md`
    /// when empty, adopted in place when it already holds engrams. Defaults to
    /// the configured domains root at `<root>/<domain>` (root default
    /// `~/Documents/Crystalline`). The default mode when neither `repo` nor
    /// `virtual` is given.
    #[serde(default)]
    pub folder: Option<String>,
    /// A database-backed virtual domain with no files on disk. Requires
    /// `domain`; cannot be combined with `repo` or `folder`.
    #[serde(rename = "virtual", default)]
    pub is_virtual: bool,
    /// A GitHub team domain: the repository, `owner/name`. Registers the
    /// repository as a local domain and downloads its knowledge to share back.
    /// Requires GitHub collaboration to be enabled first (configure
    /// github.enabled). Cannot be combined with `virtual`.
    #[serde(default)]
    pub repo: Option<String>,
    /// A subfolder within `repo` that is the domain root, for a team domain
    /// living inside a bigger repository. Defaults to the repository root.
    #[serde(default)]
    pub path: Option<String>,
    /// The branch to track, for a team domain. Defaults to `main`.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Parameters for `share_changes`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ShareChangesParams {
    /// The domain whose new knowledge to share.
    pub domain: String,
    /// A title for the shared proposal. Defaults to a generated summary.
    #[serde(default)]
    pub title: Option<String>,
    /// A longer description of what changed and why.
    #[serde(default)]
    pub description: Option<String>,
}

/// Parameters for `update_domain`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct UpdateDomainParams {
    /// The domain to bring up to date. Omit to update every shared domain.
    #[serde(default)]
    pub domain: Option<String>,
}

/// Parameters for `origin_status`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct OriginStatusParams {
    /// Restrict the review to this domain. Omit to review every shared
    /// domain.
    #[serde(default)]
    pub domain: Option<String>,
}

/// Parameters for `resolve_conflict`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ResolveConflictParams {
    /// The domain the conflict belongs to.
    pub domain: String,
    /// The domain-relative path of the flagged engram.
    pub path: String,
    /// One of mine (keep your version), theirs (take the team's version) or
    /// merged (use `content`).
    pub resolution: String,
    /// The merged markdown content. Required when `resolution` is merged.
    #[serde(default)]
    pub content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The list and map params switched to bare `Vec`/`BTreeMap` for a plain
    /// schema type must still accept an explicit `null` (which the CLI and some
    /// clients send for an empty list) and a missing key, both as empty, while
    /// a real value round-trips.
    #[test]
    fn list_and_map_params_tolerate_null_and_missing() {
        let s: SearchParams = serde_json::from_value(json!({
            "domains": null,
            "tags": null,
        }))
        .unwrap();
        assert!(s.domains.is_empty() && s.tags.is_empty());

        let c: ConfigureParams = serde_json::from_value(json!({
            "set": null,
            "unset": null,
        }))
        .unwrap();
        assert!(c.set.is_empty() && c.unset.is_empty());

        let missing: SearchParams = serde_json::from_value(json!({})).unwrap();
        assert!(missing.domains.is_empty() && missing.tags.is_empty());

        let real: SearchParams = serde_json::from_value(json!({
            "domains": ["a", "b"],
        }))
        .unwrap();
        assert_eq!(real.domains, vec!["a".to_string(), "b".to_string()]);
    }
}
