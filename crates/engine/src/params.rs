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
    /// A domain-relative subfolder to place the engram in. Defaults to the
    /// root. The folder becomes the engram's permalink prefix, so a topic
    /// filed under one is reachable as the `build_context` glob
    /// `crystalline://domain/folder/*`.
    #[serde(default)]
    pub folder: Option<String>,
    /// The engram `type`. Defaults to `engram`. Recommended values: engram,
    /// guide, decision, architecture, runbook, reference (guidance, not enforced).
    #[serde(rename = "type", default)]
    pub engram_type: Option<String>,
    /// Tags, lowercase-with-hyphens. At least one is recommended.
    #[serde(default, deserialize_with = "null_as_default")]
    pub tags: Vec<String>,
    /// Lifecycle `status`. Defaults to `stable`. Recommended values: stable,
    /// implemented, draft, proposed, idea, poc, deprecated, superseded, archived,
    /// legacy (guidance so an agent can tell an idea apart from settled fact).
    /// `current` is the legacy alias for `stable` and means the same thing; a
    /// status filter on either word matches engrams carrying either.
    #[serde(default)]
    pub status: Option<String>,
    /// Extra frontmatter keys, preserved verbatim and filterable. Temporal
    /// bounds go here: valid_from, valid_to, source_date and stale_after (when
    /// the knowledge is due a re-check) must be plain ISO dates (YYYY-MM-DD);
    /// any other value fails the write, while an explicit null or a sentinel
    /// far-future date is dropped. A verification goes here too, as
    /// verified: { by: <actor>, at: <RFC 3339 instant> } or a list of those.
    /// The write provenance keys generated and timestamp are engine-owned and
    /// are ignored when passed here, as are type, title, permalink, tags,
    /// status and recorded_at, which have their own parameters.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Replace the engram that already answers to this permalink instead of
    /// erroring. It is replaced in its own file, whatever that file is
    /// called; an engram that lives in another folder is refused (move it
    /// with move_engram first).
    #[serde(default)]
    pub overwrite: bool,
    /// Your model id, for example claude-opus-5; recorded beside who wrote it
    /// so a reader can weigh the page. Omit it only if you do not know it.
    #[serde(default)]
    pub model: Option<String>,
    /// A draft share-link somebody handed you (`dl_...`), to work inside the
    /// draft it opens instead of writing your own copy. Pass it when you were
    /// given a link and mean to compose into its author's draft: it binds the
    /// link to this account and opens the draft for this connection, and the
    /// write then lands in that author's copy, where they review it. How long
    /// that lasts depends on how you are connected: a stdio server or an MCP
    /// session keeps it until the session ends, and a sessionless HTTP
    /// connection keeps it for 30 minutes after your last call about that
    /// draft. A write at any other page while inside a draft is refused, so
    /// leave it out unless this call is about the shared page.
    #[serde(default)]
    pub share_link: Option<String>,
}

/// Parameters for `read_engram`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ReadParams {
    /// A bare permalink, title or `crystalline://` URL. Without the scheme
    /// the identifier is domain-relative: never prefix it with a domain name.
    /// A file path is not an identifier: a miss names the permalink it
    /// probably meant.
    pub identifier: String,
    /// Restrict resolution to this domain.
    #[serde(default)]
    pub domain: Option<String>,
    /// A draft share-link somebody handed you (`dl_...`), to read the draft it
    /// opens rather than the page the domain holds. Pass it the first time you
    /// are given one and on any later read of that draft; it binds the link to
    /// this account and opens the draft for this connection, so a later
    /// `edit_engram` of that page lands in its author's copy. How long that
    /// lasts depends on how you are connected: a stdio server or an MCP
    /// session keeps it until the session ends, and a sessionless HTTP
    /// connection keeps it for 30 minutes after your last call about that
    /// draft. Pass the link again whenever an edit is refused as unjoined.
    /// Leave it out everywhere else - an ordinary read never needs one, and a
    /// link you may only READ still opens the draft for reading.
    #[serde(default)]
    pub share_link: Option<String>,
}

/// Parameters for `edit_engram`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct EditParams {
    /// A bare permalink, title or `crystalline://` URL. Without the scheme
    /// the identifier is domain-relative: never prefix it with a domain name.
    /// A file path is not an identifier: a miss names the permalink it
    /// probably meant.
    pub identifier: String,
    /// The engram's domain.
    pub domain: String,
    /// One of append, prepend, find_replace, replace_section,
    /// insert_before_section, insert_after_section, set_frontmatter.
    pub operation: String,
    /// The content to add or the replacement text. For the section operations
    /// this is the section body; the heading line stays, so never repeat it
    /// (a repeated heading is dropped). Required by every operation except
    /// set_frontmatter, which takes key and value instead.
    #[serde(default)]
    pub content: Option<String>,
    /// The frontmatter field to assign, for set_frontmatter. tags takes values
    /// (a list); status, valid_from, valid_to, stale_after, source_date,
    /// resource, source_version, salience, verified and evolve_ack take value.
    /// No other key is settable here: type, title, permalink, recorded_at and
    /// the generated provenance block carry identity and provenance.
    #[serde(default)]
    pub key: Option<String>,
    /// The whole new list, for set_frontmatter on tags (add a tag, remove a
    /// tag, set tags): read the engram, then pass every tag it should carry.
    /// Each is folded to lowercase-with-hyphens and deduplicated, and a tag
    /// that cannot be folded is refused by name; an empty list removes the
    /// key.
    #[serde(default)]
    pub values: Option<Vec<String>>,
    /// The value to assign, for set_frontmatter. Omit it (or pass null) to
    /// remove the field, which is how a valid_to that should never have been
    /// set is cleared; status cannot be removed, since every engram needs one.
    /// The four date keys take a plain ISO date (YYYY-MM-DD), resource and
    /// source_version take plain text, salience a number from 0 to 10. verified
    /// is the exception: it never removes, it stamps a
    /// verification record `{ by, at }` with the current instant, taking the
    /// value as the verifying actor and falling back to the caller's own
    /// identity when it is omitted. evolve_ack takes a rule id optionally
    /// followed by a note ("V101" or "V101 lineage citation, keep") and records
    /// that the finding is intentional; the server computes what evidence it
    /// was given for.
    #[serde(default)]
    pub value: Option<String>,
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
    /// The checksum from a prior `read_engram`, guarding the edit against a
    /// change since it was read: the edit is refused as a conflict if the
    /// content changed, whichever storage kind holds it. Omit for
    /// last-write-wins.
    #[serde(default)]
    pub expected_checksum: Option<String>,
    /// Your model id, for example claude-opus-5; recorded beside who wrote it
    /// so a reader can weigh the page. Omit it only if you do not know it.
    #[serde(default)]
    pub model: Option<String>,
    /// The pair an `evolve_ack` assignment names, for the one rule that is
    /// acknowledged per pair rather than per engram.
    ///
    /// Never on the wire - `serde(skip)` keeps it out of the deserialized body
    /// and out of the tool schema alike. An agent setting frontmatter says
    /// which rule it is ruling intentional and the server works out what that
    /// rule is firing on; this is how the REST acknowledgment route passes on
    /// the pair a person's client named, where the row they clicked is the one
    /// they meant and nothing else can tell the server which of two it was.
    #[serde(skip)]
    pub ack_scope: Option<String>,
    /// A draft share-link somebody handed you (`dl_...`), to edit the draft it
    /// opens instead of your own copy of the page. Pass it when you were given
    /// a link and mean to compose into its author's draft: it binds the link
    /// to this account and opens the draft for this connection, and the edit
    /// then lands in that author's copy, where they review it. How long that
    /// lasts depends on how you are connected: a stdio server or an MCP
    /// session keeps it until the session ends, and a sessionless HTTP
    /// connection keeps it for 30 minutes after your last call about that
    /// draft - pass the link again whenever an edit is refused as unjoined. An
    /// edit of any other page while inside a draft is refused, so leave it out
    /// unless this call is about the shared page.
    #[serde(default)]
    pub share_link: Option<String>,
}

/// Parameters for `save_engram`, the full-document save behind the HTTP PUT.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SaveParams {
    /// The engram's domain.
    pub domain: String,
    /// A bare permalink, title or `crystalline://` URL. Without the scheme
    /// the identifier is domain-relative: never prefix it with a domain name.
    /// A file path is not an identifier: a miss names the permalink it
    /// probably meant.
    pub identifier: String,
    /// The complete markdown text, frontmatter included, written verbatim.
    pub content: String,
    /// The checksum from the read this save is based on. The save is refused
    /// as a conflict when the stored version no longer matches, on file and
    /// virtual domains alike.
    pub expected_checksum: String,
}

/// Parameters for `retire_engram`, the guided retirement behind the HTTP
/// retire action.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct RetireParams {
    /// The engram's domain.
    pub domain: String,
    /// A bare permalink, title or `crystalline://` URL, domain-relative.
    pub identifier: String,
    /// The retirement status: deprecated, superseded or archived. This guided
    /// flow accepts exactly these three; any other status goes through the
    /// ordinary save path.
    pub status: String,
    /// The successor's permalink or title in the same domain. Wires the
    /// superseded_by / supersedes relation pair. Required when status is
    /// superseded (T005 would flag the result otherwise), refused for the
    /// other two.
    #[serde(default)]
    pub successor: Option<String>,
    /// The date validity ends, plain ISO (YYYY-MM-DD). Omitted means the end
    /// date is unknown, never a sentinel.
    #[serde(default)]
    pub valid_to: Option<String>,
}

/// Parameters for `split_engram`, the atomic move of part of an engram into a
/// new one.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct SplitParams {
    /// The source engram's domain. The new engram lands in the same domain.
    pub domain: String,
    /// The source engram: a bare permalink, title or `crystalline://` URL,
    /// domain-relative.
    pub identifier: String,
    /// The new engram's title. Slugified into its permalink, as write_engram
    /// slugifies one.
    pub title: String,
    /// A domain-relative subfolder for the new engram. Defaults to the domain
    /// root, whatever folder the source sits in.
    #[serde(default)]
    pub folder: Option<String>,
    /// The observation bullets to move, by the one-based line numbers
    /// read_engram reports. Every line must be an observation bullet on the
    /// source.
    #[serde(default, deserialize_with = "null_as_default")]
    pub observations: Vec<usize>,
    /// The sections to move, by heading path (`## API > ### Auth`), the same
    /// form edit_engram accepts. A section moves with its heading and every
    /// deeper subsection under it.
    #[serde(default, deserialize_with = "null_as_default")]
    pub sections: Vec<String>,
    /// The checksum from a prior read of the source, guarding the split
    /// against a change since that read: both writes are refused as a conflict
    /// if the source changed. Omit for last-write-wins.
    #[serde(default)]
    pub expected_checksum: Option<String>,
}

/// Parameters for `move_engram`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct MoveParams {
    /// A bare permalink, title or `crystalline://` URL. Without the scheme
    /// the identifier is domain-relative: never prefix it with a domain name.
    /// A file path is not an identifier: a miss names the permalink it
    /// probably meant.
    pub identifier: String,
    /// The engram's current domain.
    pub domain: String,
    /// The new domain-relative path (with or without the `.md` suffix). Pass
    /// the engram's own current path to change only its permalink.
    pub destination: String,
    /// Move to a different domain. Bare links from the domain it leaves gain
    /// the domain prefix, [[domain:Target]], so they still resolve.
    #[serde(default)]
    pub destination_domain: Option<String>,
    /// The permalink the engram answers to after the move. Omit it and the
    /// permalink follows the move when it was in step with the old path (the
    /// path's own slug) and stays when it was a deliberate custom one. "path"
    /// derives it from the destination path, which is how an engram whose
    /// permalink drifted off its folder is repaired: destination = its own
    /// path, permalink = "path". "keep" keeps the current permalink whatever
    /// it is. Any other value is that permalink: a domain-relative slug such
    /// as projects/velog/alpha, with no crystalline:// scheme and no domain
    /// prefix. Refused when another engram in the destination domain already
    /// answers to it.
    #[serde(default)]
    pub permalink: Option<String>,
    /// Rewrite every reference to the moved engram when its address changes:
    /// [[old]] and [[domain:old]] links, relations and crystalline://domain/old
    /// URLs (a #fragment is kept), in every domain you can see. Defaults to
    /// true; false leaves them dangling.
    #[serde(default)]
    pub update_links: Option<bool>,
}

/// Parameters for `delete_engram`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DeleteParams {
    /// A bare permalink, title or `crystalline://` URL. Without the scheme
    /// the identifier is domain-relative: never prefix it with a domain name.
    /// A file path is not an identifier: a miss names the permalink it
    /// probably meant.
    pub identifier: String,
    /// The engram's domain.
    pub domain: String,
    /// The checksum from a prior read. When supplied, the delete is refused as
    /// a conflict if the engram changed since that read. Omit for an
    /// unconditional delete (the MCP tool's existing behavior).
    #[serde(default)]
    pub expected_checksum: Option<String>,
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
    /// Filter by `status`. `stable` and `current` mean the same state, so a
    /// filter on either word matches engrams carrying either; every other value
    /// matches exactly.
    #[serde(default)]
    pub status: Option<String>,
    /// Frontmatter filters, `{ key: value }` or `{ key: { $gt: n } }`. The
    /// filterable keys are the promoted ones (type, status, title, permalink,
    /// recorded_at, valid_from, valid_to, tags, plus timestamp for the write
    /// instant), every custom frontmatter key an engram carries (salience among
    /// them) and source_date, last_verified, stale_after, temporal_confidence,
    /// resource and verified. last_verified and stale_after carry the effective
    /// value whichever spelling an engram uses, so review_after is not a filter
    /// key of its own; the generated block's by actor is not filterable either.
    /// A status filter here matches exactly, so use the status parameter when
    /// you want the stable and current spellings folded together.
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
    /// Maximum related engrams beyond the anchors, keeping the top-ranked. Defaults to 10.
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
    /// Also report schema drift: observation categories and relation types
    /// in use but undeclared by the schema, and declared but unused.
    #[serde(default)]
    pub drift: bool,
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

/// Parameters for `vocabulary`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct VocabularyParams {
    /// Restrict to one domain. Omit for a vocabulary across every domain.
    #[serde(default)]
    pub domain: Option<String>,
}

/// Parameters for `evolve_engrams`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct EvolveParams {
    /// Restrict the sweep to these domains. Omit to sweep every registered
    /// domain.
    #[serde(default, deserialize_with = "null_as_default")]
    pub domains: Vec<String>,
    /// Restrict the sweep to these detector families: temporal (validity
    /// windows, staleness and the supersede lifecycle), structure (references,
    /// reciprocity, orphans, stubs and size), redundancy (duplicate content,
    /// colliding titles and tag drift) or meaning (semantic twins and possible
    /// contradictions). Omit for all four.
    #[serde(default, deserialize_with = "null_as_default")]
    pub families: Vec<String>,
    /// Restrict the sweep to these rule ids, for example V001 or V201. Omit for
    /// every rule in the requested families.
    #[serde(default, deserialize_with = "null_as_default")]
    pub rules: Vec<String>,
    /// Drop findings scoring under this priority, 0 to 100. Useful to see only
    /// what matters most on a large archive.
    #[serde(default)]
    pub min_priority: Option<u8>,
    /// Page size. Defaults to 10, capped at 100.
    #[serde(default)]
    pub limit: Option<usize>,
    /// One-based page number. Defaults to 1.
    #[serde(default)]
    pub page: Option<usize>,
    /// Evaluate the temporal rules as of this ISO date (YYYY-MM-DD) instead of
    /// today, so a run is reproducible. Only the date comparisons move; nothing
    /// else about the sweep changes.
    #[serde(default)]
    pub today: Option<String>,
    /// Include the findings acknowledgments suppressed, each marked
    /// acknowledged with the scope and note that silenced it. Off by default:
    /// the queue is what still needs deciding, and this is the audit view of
    /// what was already decided.
    #[serde(default)]
    pub include_acknowledged: bool,
}

/// Parameters for `configure`. Omit everything to see the current settings,
/// and with `github.enabled` on the GitHub connection too; with it off the
/// snapshot reports that the feature is off and how to turn it on, leaving
/// the stored credential unread. `token` or `connect` handle a GitHub
/// connect action on their own and ignore `set`/`unset` in the same call;
/// give them on a separate call from a settings change.
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
    /// confirm at github.com/login/device, then click Authorize on the page
    /// that follows; the result carries next_steps to relay verbatim, and
    /// calling configure again reports whether the sign-in landed. Omit
    /// when `token` is supplied. Works whether or not github.enabled is on
    /// yet; enabling is only needed for team domains.
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
    /// With connect: github, abandon a pending sign-in and start a fresh
    /// code.
    #[serde(default)]
    pub restart: bool,
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
    /// The branch to track, for a team domain. Defaults to the repository's
    /// default branch, asked from GitHub when the domain is added.
    #[serde(default)]
    pub branch: Option<String>,
}

/// Parameters for `remove_domain`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RemoveDomainParams {
    /// The registered domain to unregister.
    pub domain: String,
    /// Required to unregister a VIRTUAL domain, whose engrams live in the
    /// database and are deleted with it. Ignored for a file domain, whose
    /// files are never touched either way.
    #[serde(default)]
    pub purge: bool,
    /// Every OTHER actor holding private drafts in this domain, by name.
    /// Required when anybody but you is drafting here: a removal ends their
    /// unshared work and nothing brings it back, so it is named rather than
    /// assumed. The refusal says who, and how many drafts each of them holds.
    /// Your own drafts need no naming, and naming yourself as well is taken
    /// and changes nothing - so the actor list a removal preview or a
    /// refusal reports can be sent back as it stands.
    #[serde(default)]
    pub end_drafts: Vec<String>,
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
    /// Amend this open proposal (layer) instead of stacking a new one - the
    /// way to answer its review feedback.
    #[serde(default)]
    pub proposal: Option<u64>,
    /// Share only these changed files (domain-relative); omitted shares every
    /// unshared change. Folder indexes of the affected folders ride along.
    #[serde(default)]
    pub files: Option<Vec<String>>,
}

/// Parameters for `discard_changes`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct DiscardChangesParams {
    /// The team domain the changes belong to.
    pub domain: String,
    /// The domain-relative paths to put back the way the team has them.
    pub paths: Vec<String>,
    /// Optional guard, path to the SHA-256 hex of the current content you
    /// looked at (the `sha` origin_status reports with diff: true). A path
    /// whose content moved since is refused as changed_since. Omit it to
    /// discard each named path unconditionally.
    #[serde(default)]
    pub expected: Option<std::collections::BTreeMap<String, String>>,
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
    /// Name the unshared files instead of only counting them: each domain
    /// then carries a detail block listing the changed paths grouped as
    /// added, modified and deleted, plus how many generated folder listings
    /// ride along. Ask for it whenever you have to say WHAT is unshared;
    /// leave it off when the count is all you need.
    #[serde(default)]
    pub detail: bool,
    /// With detail, also return both sides of every unshared file: the
    /// team's copy and yours, so you can say what changed before sharing or
    /// discarding. Requires domain, and implies detail.
    #[serde(default)]
    pub diff: bool,
}

/// Parameters for `resolve_conflict`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ResolveConflictParams {
    /// The domain the conflict belongs to.
    pub domain: String,
    /// The domain-relative path of the flagged engram.
    pub path: String,
    /// One of mine (keep your version), theirs (take the team's version) or
    /// merged (use `content`). On a 2026-07-28 peer that declared an
    /// elicitation capability it may be omitted: the call then answers with a
    /// mine-or-theirs question previewing both sides. merged cannot be chosen
    /// through the question - call again with resolution merged and content.
    #[serde(default)]
    pub resolution: Option<String>,
    /// The merged markdown content. Required when `resolution` is merged.
    #[serde(default)]
    pub content: Option<String>,
}

/// Parameters for `withdraw_proposal`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WithdrawProposalParams {
    /// The team domain the proposal belongs to.
    pub domain: String,
    /// The proposal number. Omit to withdraw the domain's single open
    /// proposal.
    #[serde(default)]
    pub proposal: Option<u64>,
    /// Also restore the shared files to their pre-share content (files
    /// edited since sharing are left alone). Default false: the knowledge
    /// stays local, only the proposal goes away.
    #[serde(default)]
    pub revert: Option<bool>,
}

/// Parameters for `skills`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SkillsParams {
    /// The skill to read in full. Omit for the index of every shipped skill
    /// with its description.
    #[serde(default)]
    pub name: Option<String>,
}

/// Parameters for `provision`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ProvisionParams {
    /// What to do: "status" reports decisions, shipped artifacts and installed
    /// state; "allow" or "deny" records the user's decision for `domain` and
    /// applies it; "apply" reconciles current decisions.
    pub action: String,
    /// The domain for allow or deny. Omit for status and apply.
    #[serde(default)]
    pub domain: Option<String>,
}

/// A params struct that names existing domains, so a caller may spell each
/// one as its local name, its canonical name or an alias.
///
/// Only references to domains that already exist implement this. A name a
/// registration introduces (`add_domain`'s `domain`, a rename's new name) is
/// not a reference: mapping it through the table would let a new domain adopt
/// or collide with the domain an alias already points at, so those params keep
/// what the caller typed.
pub trait DomainArgs {
    /// Map every domain spelling this value carries to a local name.
    fn localize_domains(&mut self, local: &dyn Fn(&str) -> String);
}

/// A `crystalline://` identifier with its domain segment localized; any other
/// text unchanged, the permalink, a fragment and a glob byte for byte.
pub fn localize_identifier(identifier: &str, local: &dyn Fn(&str) -> String) -> String {
    let Some(rest) = identifier.strip_prefix(crystalline_core::address::SCHEME) else {
        return identifier.to_string();
    };
    let (domain, tail) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    if domain.is_empty() {
        return identifier.to_string();
    }
    format!(
        "{}{}{tail}",
        crystalline_core::address::SCHEME,
        local(domain)
    )
}

fn localize_one(value: &mut String, local: &dyn Fn(&str) -> String) {
    *value = local(value);
}

fn localize_opt(value: &mut Option<String>, local: &dyn Fn(&str) -> String) {
    if let Some(value) = value {
        localize_one(value, local);
    }
}

fn localize_all(values: &mut [String], local: &dyn Fn(&str) -> String) {
    for value in values {
        localize_one(value, local);
    }
}

fn localize_id(identifier: &mut String, local: &dyn Fn(&str) -> String) {
    *identifier = localize_identifier(identifier, local);
}

/// `DomainArgs` for a params struct, naming which fields hold a domain name
/// (`one`, `opt`, `all`) and which hold an identifier (`id`, `id_opt`).
macro_rules! domain_args {
    ($ty:ty { $($kind:ident $field:ident),* $(,)? }) => {
        impl DomainArgs for $ty {
            fn localize_domains(&mut self, local: &dyn Fn(&str) -> String) {
                $(domain_args!(@field $kind, self.$field, local);)*
            }
        }
    };
    (@field one, $value:expr, $local:ident) => { localize_one(&mut $value, $local) };
    (@field opt, $value:expr, $local:ident) => { localize_opt(&mut $value, $local) };
    (@field all, $value:expr, $local:ident) => { localize_all(&mut $value, $local) };
    (@field id, $value:expr, $local:ident) => { localize_id(&mut $value, $local) };
    (@field id_opt, $value:expr, $local:ident) => {
        if let Some(identifier) = &mut $value {
            localize_id(identifier, $local);
        }
    };
}

domain_args!(WriteParams { one domain });
domain_args!(ReadParams { id identifier, opt domain });
domain_args!(EditParams { id identifier, one domain });
domain_args!(SaveParams { one domain, id identifier });
domain_args!(RetireParams { one domain, id identifier, id_opt successor });
domain_args!(SplitParams { one domain, id identifier });
domain_args!(MoveParams { id identifier, one domain, opt destination_domain });
domain_args!(DeleteParams { id identifier, one domain });
domain_args!(SearchParams { all domains });
domain_args!(ContextParams { id anchor, all domains });
domain_args!(RecentParams { all domains });
domain_args!(BrowseParams { one domain });
domain_args!(ValidateParams { one domain, id_opt identifier });
domain_args!(InferParams { one domain });
domain_args!(VocabularyParams { opt domain });
domain_args!(EvolveParams { all domains });
domain_args!(RemoveDomainParams { one domain });
domain_args!(ShareChangesParams { one domain });
domain_args!(DiscardChangesParams { one domain });
domain_args!(UpdateDomainParams { opt domain });
domain_args!(OriginStatusParams { opt domain });
domain_args!(ResolveConflictParams { one domain });
domain_args!(WithdrawProposalParams { one domain });
domain_args!(ProvisionParams { opt domain });

// Every text a tool parameter carries may arrive with CRLF line endings, and is
// taken as the LF text it means: what Crystalline stores is LF only, and a
// find_text or a section heading sent with CRLF has to match that LF text. The
// conversion is `crystalline_core::to_lf`, applied once where each write verb
// takes its parameters.

fn lf(text: &mut String) {
    if let std::borrow::Cow::Owned(converted) = crystalline_core::to_lf(text) {
        *text = converted;
    }
}

fn lf_opt(text: &mut Option<String>) {
    if let Some(text) = text {
        lf(text);
    }
}

impl WriteParams {
    /// These parameters with every text in them as LF.
    pub(crate) fn lf_only(mut self) -> Self {
        lf(&mut self.content);
        self
    }
}

impl EditParams {
    /// These parameters with every text in them as LF.
    pub(crate) fn lf_only(mut self) -> Self {
        lf_opt(&mut self.content);
        lf_opt(&mut self.value);
        lf_opt(&mut self.section);
        lf_opt(&mut self.find_text);
        if let Some(values) = &mut self.values {
            values.iter_mut().for_each(lf);
        }
        self
    }
}

impl SaveParams {
    /// These parameters with the document as LF.
    pub(crate) fn lf_only(mut self) -> Self {
        lf(&mut self.content);
        self
    }
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

    /// The table a test localizes through: `eng` and `old-eng` both mean
    /// `eng-knowledge`, anything else stays as typed.
    fn eng(spelling: &str) -> String {
        match spelling {
            "eng" | "old-eng" => "eng-knowledge".to_string(),
            other => other.to_string(),
        }
    }

    #[test]
    fn an_identifier_keeps_everything_but_its_domain_segment() {
        assert_eq!(
            localize_identifier("crystalline://eng/runbook#Steps", &eng),
            "crystalline://eng-knowledge/runbook#Steps"
        );
        assert_eq!(
            localize_identifier("crystalline://old-eng/ops/*", &eng),
            "crystalline://eng-knowledge/ops/*"
        );
        assert_eq!(
            localize_identifier("crystalline://eng", &eng),
            "crystalline://eng-knowledge"
        );
        // Unknown spellings, bare permalinks and an empty domain stay as typed.
        assert_eq!(
            localize_identifier("crystalline://nobody/x", &eng),
            "crystalline://nobody/x"
        );
        assert_eq!(localize_identifier("eng/runbook", &eng), "eng/runbook");
        assert_eq!(
            localize_identifier("crystalline:///x", &eng),
            "crystalline:///x"
        );
    }

    #[test]
    fn params_localize_every_domain_they_name() {
        let mut read = ReadParams {
            identifier: "crystalline://eng/runbook".to_string(),
            domain: Some("old-eng".to_string()),
            ..ReadParams::default()
        };
        read.localize_domains(&eng);
        assert_eq!(read.identifier, "crystalline://eng-knowledge/runbook");
        assert_eq!(read.domain.as_deref(), Some("eng-knowledge"));

        let mut search: SearchParams =
            serde_json::from_value(json!({ "domains": ["eng", "nobody"] })).unwrap();
        search.localize_domains(&eng);
        assert_eq!(search.domains, vec!["eng-knowledge", "nobody"]);

        let mut moved: MoveParams = serde_json::from_value(json!({
            "identifier": "runbook",
            "domain": "eng",
            "destination": "ops",
            "destination_domain": "old-eng",
        }))
        .unwrap();
        moved.localize_domains(&eng);
        assert_eq!(moved.domain, "eng-knowledge");
        assert_eq!(moved.destination_domain.as_deref(), Some("eng-knowledge"));
        // The destination is a folder, never a domain.
        assert_eq!(moved.destination, "ops");

        let mut context: ContextParams = serde_json::from_value(json!({
            "anchor": "crystalline://old-eng/ops/*",
            "domains": ["eng"],
        }))
        .unwrap();
        context.localize_domains(&eng);
        assert_eq!(context.anchor, "crystalline://eng-knowledge/ops/*");
        assert_eq!(context.domains, vec!["eng-knowledge"]);
    }
}
