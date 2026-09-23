//! What a client sees before it sees any engram: which domains this instance
//! serves, what each one holds, and what each one is for.
//!
//! The listing and the tree pass the engine's own JSON through unchanged, so
//! the MCP tools and this API answer with one payload rather than two shapes
//! that drift. The manifest endpoint is the one wrapper here, because a
//! markdown document is not JSON: it is handed over as a string beside the
//! domain it belongs to.

use axum::Json;
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, ETAG};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use super::auth::Identity;
use super::{
    ApiError, ApiJson, ApiPath, ApiQuery, ConflictDetail, ProblemDetail, REVALIDATE, RestState,
    if_match, if_none_match_matches, precondition_failed, require_domain_read,
};
use crate::engine::EngineError;
use crate::params::{BrowseParams, ListDomainsParams};
use crystalline_core::{
    Manifest, ProblemKind, TagAliasProblemKind, parse_engram, policy_registry, starter_stanzas,
};

/// `GET /domains` - every registered domain with its counts, its kind and its
/// routing bullets, plus the behavior rules that govern them.
///
/// `include_routing` is always on rather than a query parameter: a browser
/// client is exactly the caller that has no other way to learn what a domain is
/// for, and the bullets are a handful of lines per domain.
#[utoipa::path(
    get,
    path = "/api/v1/domains",
    tag = "domains",
    operation_id = "list_domains",
    responses(
        (
            status = 200,
            description = "The engine's own domain listing, unchanged.\n\nA \
                           domain that reviews changes before they land \
                           carries `review: \"overlay\"` and, for a caller \
                           with an account, `my_drafts`: how many draft \
                           changes of theirs are waiting to be shared. Both \
                           are absent on a domain that takes changes directly, \
                           and `my_drafts` is absent rather than zero when \
                           there is no account to count for - a client reads \
                           presence, since null and 0 are different facts.",
            body = Object,
            example = json!({
                "behavior": [
                    "Search before answering from memory.",
                    "Record what was learned as an engram."
                ],
                "domains": [{
                    "name": "eng",
                    "kind": "file",
                    "path": "/Users/ada/Documents/Crystalline/eng",
                    "engrams": 4,
                    "observations": 12,
                    "relations": 3,
                    "last_sync": "2026-08-05T09:14:22Z",
                    "private": false,
                    "when_to_use": ["Route here for eng questions."]
                }]
            }),
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn list(
    State(state): State<RestState>,
    identity: Identity,
) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .list_domains(
            &ListDomainsParams {
                include_routing: true,
            },
            &identity.scope(),
        )
        .await?;
    Ok(Json(value))
}

/// The query string `GET /domains/{domain}/tree` takes, mirroring
/// [`BrowseParams`] minus the domain the path already names.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TreeQuery {
    /// A domain-relative folder path. Defaults to the root.
    #[serde(default)]
    #[param(example = "notes")]
    path: Option<String>,
    /// How many folder levels deep to list. Defaults to 1. The `total` in the
    /// answer counts this depth, so it moves with this parameter and is not
    /// the folder's recursive size.
    #[serde(default)]
    #[param(example = 2)]
    depth: Option<usize>,
    /// A glob filtering the engram paths listed.
    #[serde(default)]
    #[param(example = "notes/**")]
    glob: Option<String>,
}

/// `GET /domains/{domain}/tree` - one domain's engrams and subfolders under a
/// path, the navigation a file tree in the UI is built from.
///
/// One level at a time and bounded: see [`crate::engine::TREE_LEVEL_CAP`] for
/// what a level that does not fit answers with.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/tree",
    tag = "domains",
    operation_id = "get_domain_tree",
    summary = "One level of a domain's folders and engrams.",
    description = "The navigation a file tree is built from, one level at a \
                   time and bounded: a level holding more engrams than the \
                   tree shows is cut, `total` says how many it really holds \
                   and `truncated` says the rows were cut, so a client can \
                   send its reader to the paged listing instead.\n\n`total` \
                   counts this level, not the folder: it moves with `depth` \
                   and leaves out everything nested deeper, so a folder of ten \
                   engrams holding a subfolder of a thousand reports ten here. \
                   `GET /domains/{domain}/engrams?path=...` counts the same \
                   folder recursively and reports the larger number. Neither \
                   is the other's approximation - a level states a fact about \
                   the rows it drew, a folder listing promises the folder - so \
                   a client that means to say \"N engrams in this folder\" \
                   takes that number from the listing.\n\n`folders` \
                   is never cut, so a truncated level still names every folder \
                   a reader can descend into. A `glob` narrows the engrams \
                   this level returned, so on a truncated level it selects \
                   within the cut rather than across the whole folder, and it \
                   does not filter `folders` at all.",
    params(("domain" = String, Path, description = "The registered domain."), TreeQuery),
    responses(
        (
            status = 200,
            description = "The engine's own browse payload, unchanged.",
            body = Object,
            example = json!({
                "domain": "eng",
                "path": "/",
                "folders": ["notes"],
                "engrams": [{
                    "permalink": "alpha",
                    "title": "Alpha",
                    "type": "engram",
                    "status": "stable",
                    "path": "alpha.md"
                }],
                "truncated": false,
                "total": 1
            }),
        ),
        (
            status = 400,
            description = "The query string will not parse.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The glob is not a valid pattern.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn tree(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiQuery(query): ApiQuery<TreeQuery>,
) -> Result<Json<Value>, ApiError> {
    let value = state
        .engine
        .browse_domain(
            &BrowseParams {
                domain,
                path: query.path,
                depth: query.depth,
                glob: query.glob,
            },
            &identity.scope(),
        )
        .await?;
    Ok(Json(value))
}

/// `GET /domains/{domain}/manifest` - the domain's MANIFEST markdown as
/// written, so a client can render or edit the source rather than a reduction
/// of it.
///
/// The response carries an `ETag` over the markdown, the same strong
/// validator [`save_manifest`] compares an `If-Match` against, so a client
/// that means to edit the manifest can go straight from this read to that
/// write without a second round trip.
#[utoipa::path(
    get,
    path = "/api/v1/domains/{domain}/manifest",
    tag = "domains",
    operation_id = "get_domain_manifest",
    summary = "The domain's MANIFEST markdown as written.",
    description = "The source, not a reduction of it, so a client can render \
                   or edit it directly.\n\nThe response carries an `ETag` \
                   over the markdown, the same strong validator a later \
                   `PUT` compares an `If-Match` against. `If-None-Match` \
                   naming the current checksum answers 304 with no body, and \
                   `Cache-Control: no-cache` on both the 200 and the 304 keeps \
                   a stored copy revalidating instead of going heuristically \
                   fresh, so a save elsewhere is picked up on its next \
                   use.\n\n`sections` is what the core crate reads out of the \
                   source: the routing bullets and which of them an agent \
                   reads, the provisioning and tag alias declarations with \
                   every bullet that did not parse, and every frontmatter \
                   policy key with what it declares and what holds. `null` \
                   for `provisioning` or `tag_aliases` means the section is \
                   absent.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        (
            "If-None-Match" = Option<String>,
            Header,
            description = "The quoted checksum of a version already held. A \
                           match answers 304 with no body.",
            example = "\"3f8a1c05e2\"",
        ),
    ),
    responses(
        (
            status = 200,
            description = "The manifest source beside the domain it belongs to.",
            body = ManifestResponse,
            headers(
                ("etag" = String, description = "The quoted checksum of \
                 the manifest as read, the token a later `PUT` carries \
                 in `If-Match`."),
                ("cache-control" = String, description = "Always `no-cache`: \
                 store it, but revalidate before every use."),
            ),
            example = json!({
                "domain": "eng",
                "markdown": "---\ntitle: eng\n---\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions.\n",
                "checksum": "3f8a1c05e2",
                "sections": {
                    "scope": ["Everything about eng"],
                    "when_to_use": ["Route here for eng questions."],
                    "routing": "when_to_use",
                    "missing": [],
                    "provisioning": null,
                    "tag_aliases": null,
                    "policies": [
                        { "key": "generated_indexes", "declared": null, "effective": "local", "values": ["local", "shared"], "default": "local", "meaning": "Whether the generated folder listings travel with a share.", "changed_by": "owner" },
                        { "key": "sharing", "declared": null, "effective": "proposal", "values": ["proposal", "direct"], "default": "proposal", "meaning": "Whether a share opens a proposal for review or commits straight to the branch.", "changed_by": "owner" }
                    ],
                    "starters": [
                        { "section": "When to Use", "meaning": "Agents pick this domain by these bullets. Without one, nothing routes here.", "example": "## When to Use\n\n- Route here for questions about how our deployment pipeline works" },
                        { "section": "Scope", "meaning": "What belongs in this domain and what does not. Read for routing only when When to Use is empty.", "example": "## Scope\n\n- Infrastructure and deployment, not application code" },
                        { "section": "Provisioning", "meaning": "Folders this domain installs into an AI harness: skills, commands, agents or MCP configs.", "example": "## Provisioning\n\n- skills: skills" },
                        { "section": "Tag Aliases", "meaning": "Spellings that fold into one canonical tag, so a search for either finds both.", "example": "## Tag Aliases\n\n- k8s -> kubernetes" }
                    ]
                }
            }),
        ),
        (
            status = 304,
            description = "`If-None-Match` names the current checksum; no body \
                           is sent. Carries the `ETag` it matched and the same \
                           `Cache-Control`.",
        ),
        (
            status = 401,
            description = "No identity.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The trusted-header identity names a disabled account.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or the domain carries no MANIFEST yet.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn manifest(
    State(state): State<RestState>,
    identity: Identity,
    headers: HeaderMap,
    ApiPath(domain): ApiPath<String>,
) -> Result<Response, ApiError> {
    // A MANIFEST is a domain's own text - its scope, its routing bullets -
    // so a caller who may not see the domain may not read it either, and is
    // told what a caller asking for a domain nobody registered is told.
    require_domain_read(&state, &identity, &domain).await?;
    let markdown = state.engine.manifest_markdown(&domain).await?;
    let checksum = manifest_checksum(&markdown);
    if if_none_match_matches(&headers, &checksum) {
        let etag = HeaderValue::from_str(&format!("\"{checksum}\""))
            .map_err(|_| ApiError::internal("the manifest's checksum is not a usable ETag"))?;
        // The validator and `Cache-Control`, no body: the shape is stated
        // once, on `if_none_match_matches`.
        return Ok((
            StatusCode::NOT_MODIFIED,
            [
                (ETAG, etag),
                (CACHE_CONTROL, HeaderValue::from_static(REVALIDATE)),
            ],
        )
            .into_response());
    }
    let mut resp = manifest_response(&domain, markdown, StatusCode::OK, None)?;
    resp.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(REVALIDATE));
    Ok(resp)
}

/// What `GET` and `PUT /domains/{domain}/manifest` answer with: the source,
/// its checksum and the features the core crate reads out of it.
#[derive(Debug, Serialize, ToSchema)]
#[schema(description = "The MANIFEST source beside the domain it belongs to, \
                        its checksum, and the features parsed out of it: \
                        what an agent routes by, what the domain provisions, \
                        which tags fold into which, and every frontmatter \
                        policy key with what it declares and what holds.")]
pub struct ManifestResponse {
    /// The domain the MANIFEST introduces.
    #[schema(example = "eng")]
    pub domain: String,
    /// The MANIFEST markdown as written, frontmatter included.
    pub markdown: String,
    /// sha256 of the markdown, the token a later `PUT` carries in `If-Match`.
    #[schema(example = "3f8a1c05e2")]
    pub checksum: String,
    /// The features read out of the markdown.
    pub sections: ManifestSections,
}

/// The MANIFEST's features as the core crate reads them. Nothing here is
/// interpreted a second time, and a change goes through the editor: this is
/// a view of the source beside it.
#[derive(Debug, Serialize, ToSchema)]
pub struct ManifestSections {
    /// The `Scope` bullets; empty when the section is absent or empty.
    pub scope: Vec<String>,
    /// The `When to Use` bullets; empty when the section is absent or empty.
    pub when_to_use: Vec<String>,
    /// Which of the two an agent reads: `when_to_use`, or `scope` when When
    /// to Use is absent or empty, or `none` when both are.
    pub routing: RoutingSource,
    /// The required sections the MANIFEST lacks, by name: `Scope`, `When to
    /// Use`. Empty when both are there.
    pub missing: Vec<String>,
    /// The `Provisioning` section, or `null` when the MANIFEST has none.
    pub provisioning: Option<ProvisioningView>,
    /// The `Tag Aliases` section, or `null` when the MANIFEST has none.
    pub tag_aliases: Option<TagAliasesView>,
    /// Every MANIFEST configuration key the core crate knows, with what the
    /// frontmatter declares and what holds. Drawn from the policy registry, so
    /// a key added there appears here without a line of this file changing.
    /// Empty for a MANIFEST that did not parse: a document nobody can read
    /// declares nothing.
    pub policies: Vec<PolicyView>,
    /// Every MANIFEST section that carries meaning, with a line on what it
    /// does and an example that can be saved as it stands. Drawn from the
    /// core registry rather than from this document, so it is sent whether or
    /// not the MANIFEST declares any of them - and sent for a MANIFEST that
    /// did not parse too, which is the one a reader most needs explained.
    pub starters: Vec<StarterStanzaView>,
}

/// Which routing section an agent reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoutingSource {
    /// The `When to Use` bullets.
    WhenToUse,
    /// The `Scope` bullets, because `When to Use` is absent or empty.
    Scope,
    /// Nothing: both are absent or empty, and no agent can route here.
    None,
}

/// The `Provisioning` section: what parsed, and what did not.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProvisioningView {
    /// The declarations that parsed, in document order, one per kind.
    pub decls: Vec<ProvisioningDeclView>,
    /// The bullets that did not parse, or lost to an earlier duplicate.
    pub problems: Vec<ManifestProblem>,
}

/// One `kind: path` declaration.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProvisioningDeclView {
    /// `skills`, `commands`, `agents` or `mcps`.
    #[schema(example = "skills")]
    pub kind: String,
    /// The folder, relative to the MANIFEST, trailing slash trimmed.
    #[schema(example = "skills")]
    pub path: String,
}

/// A bullet the core crate flagged, kept verbatim beside why.
#[derive(Debug, Serialize, ToSchema)]
pub struct ManifestProblem {
    /// The category, in snake case: `malformed`, `unknown_type`,
    /// `invalid_path`, `duplicate_type`, `self_alias`, `duplicate_alias`,
    /// `non_canonical_target`, `chained_alias`.
    #[schema(example = "unknown_type")]
    pub kind: String,
    /// The bullet as written, without its dash.
    #[schema(example = "widgets: w")]
    pub bullet: String,
    /// Why it was flagged, in the crate's words.
    pub reason: String,
}

/// The `Tag Aliases` section: the mappings kept, and the bullets flagged.
#[derive(Debug, Serialize, ToSchema)]
pub struct TagAliasesView {
    /// The mappings kept, in document order.
    pub decls: Vec<TagAliasDeclView>,
    /// The bullets flagged. A non-canonical target or a chained alias is in
    /// both lists: kept, and flagged.
    pub problems: Vec<ManifestProblem>,
}

/// One `old -> canonical` mapping, both sides verbatim.
#[derive(Debug, Serialize, ToSchema)]
pub struct TagAliasDeclView {
    #[schema(example = "Multi_Word")]
    pub alias: String,
    #[schema(example = "multi-word")]
    pub canonical: String,
}

/// One MANIFEST policy key: the registry row beside what this MANIFEST says.
#[derive(Debug, Serialize, ToSchema)]
pub struct PolicyView {
    /// The frontmatter key.
    #[schema(example = "sharing")]
    pub key: String,
    /// The value as the frontmatter writes it, or `null` when the key is absent.
    #[schema(example = "direct")]
    pub declared: Option<String>,
    /// The value that holds: absent and unrecognized both fall to `default`.
    #[schema(example = "direct")]
    pub effective: String,
    /// The values the key takes, in display order.
    pub values: Vec<String>,
    /// What an absent or unrecognized declaration is read as.
    #[schema(example = "proposal")]
    pub default: String,
    /// One line, present tense.
    pub meaning: String,
    /// Who may change it: `owner` (the domain's owner or an instance admin) or `admin`.
    #[schema(example = "owner")]
    pub changed_by: String,
}

/// One MANIFEST section a reader can start: what it is for, and markdown
/// that parses as it stands.
#[derive(Debug, Serialize, ToSchema)]
pub struct StarterStanzaView {
    /// The H2 heading, as the parser matches it.
    #[schema(example = "Tag Aliases")]
    pub section: String,
    /// One line, present tense: what the section does.
    pub meaning: String,
    /// Markdown a person can save as it stands. The core crate's guard test
    /// parses every one of these back into the declaration it advertises.
    #[schema(example = "## Tag Aliases\n\n- k8s -> kubernetes")]
    pub example: String,
}

/// The startable sections, straight off the core registry. Nothing about this
/// document is read: the point of the list is what a MANIFEST CAN say, which
/// is the same list for every domain and for a document that will not parse.
fn starters() -> Vec<StarterStanzaView> {
    starter_stanzas()
        .iter()
        .map(|stanza| StarterStanzaView {
            section: stanza.section.to_string(),
            meaning: stanza.meaning.to_string(),
            example: stanza.example.to_string(),
        })
        .collect()
}

/// The registry rows, joined with what `manifest` declares. A key the manifest
/// does not know is at its registry default, which is what an absent
/// declaration means.
fn policies_of(manifest: &Manifest) -> Vec<PolicyView> {
    policy_registry()
        .iter()
        .map(|spec| {
            let (declared, effective) = manifest.policy(spec.key).unwrap_or((None, spec.default));
            PolicyView {
                key: spec.key.to_string(),
                declared: declared.map(str::to_string),
                effective: effective.to_string(),
                values: spec.values.iter().map(|v| v.to_string()).collect(),
                default: spec.default.to_string(),
                meaning: spec.meaning.to_string(),
                changed_by: spec.changed_by.as_str().to_string(),
            }
        })
        .collect()
}

impl ManifestSections {
    /// The features of `markdown`, read the way the engine reads every
    /// MANIFEST. A source the format layer will not parse - no frontmatter,
    /// a broken block - has no sections to speak of, and says so as a
    /// MANIFEST lacking both required sections rather than as a failure of
    /// the read: the markdown beside it is still the thing to fix, in the
    /// editor.
    pub fn of(markdown: &str) -> ManifestSections {
        let Ok(engram) = parse_engram(markdown) else {
            return ManifestSections {
                scope: Vec::new(),
                when_to_use: Vec::new(),
                routing: RoutingSource::None,
                missing: vec!["Scope".to_string(), "When to Use".to_string()],
                provisioning: None,
                tag_aliases: None,
                // Not the registry defaults: nothing here was declared, and
                // nothing here can be, until the document parses again.
                policies: Vec::new(),
                // The starters are not read out of the document, so they
                // survive a document that cannot be read: a reader looking at
                // a broken MANIFEST still learns what it can be made to say.
                starters: starters(),
            };
        };
        let manifest = Manifest::from_engram(&engram, markdown);
        let routing = if !manifest.when_to_use().is_empty() {
            RoutingSource::WhenToUse
        } else if !manifest.scope().is_empty() {
            RoutingSource::Scope
        } else {
            RoutingSource::None
        };
        ManifestSections {
            scope: manifest.scope().to_vec(),
            when_to_use: manifest.when_to_use().to_vec(),
            routing,
            missing: manifest
                .missing_required_sections()
                .iter()
                .map(|name| name.to_string())
                .collect(),
            provisioning: manifest.provisioning().map(|section| ProvisioningView {
                decls: section
                    .decls
                    .iter()
                    .map(|decl| ProvisioningDeclView {
                        kind: decl.kind.id().to_string(),
                        path: decl.path.clone(),
                    })
                    .collect(),
                problems: section
                    .problems
                    .iter()
                    .map(|problem| ManifestProblem {
                        kind: provisioning_problem_kind(problem.kind).to_string(),
                        bullet: problem.bullet.clone(),
                        reason: problem.reason.clone(),
                    })
                    .collect(),
            }),
            tag_aliases: manifest.tag_aliases().map(|section| TagAliasesView {
                decls: section
                    .decls
                    .iter()
                    .map(|decl| TagAliasDeclView {
                        alias: decl.alias.clone(),
                        canonical: decl.canonical.clone(),
                    })
                    .collect(),
                problems: section
                    .problems
                    .iter()
                    .map(|problem| ManifestProblem {
                        kind: tag_alias_problem_kind(problem.kind).to_string(),
                        bullet: problem.bullet.clone(),
                        reason: problem.reason.clone(),
                    })
                    .collect(),
            }),
            policies: policies_of(&manifest),
            starters: starters(),
        }
    }
}

/// The wire spelling of a provisioning problem's kind.
fn provisioning_problem_kind(kind: ProblemKind) -> &'static str {
    match kind {
        ProblemKind::Malformed => "malformed",
        ProblemKind::UnknownType => "unknown_type",
        ProblemKind::InvalidPath => "invalid_path",
        ProblemKind::DuplicateType => "duplicate_type",
    }
}

/// The wire spelling of a tag alias problem's kind.
fn tag_alias_problem_kind(kind: TagAliasProblemKind) -> &'static str {
    match kind {
        TagAliasProblemKind::Malformed => "malformed",
        TagAliasProblemKind::SelfAlias => "self_alias",
        TagAliasProblemKind::DuplicateAlias => "duplicate_alias",
        TagAliasProblemKind::NonCanonicalTarget => "non_canonical_target",
        TagAliasProblemKind::ChainedAlias => "chained_alias",
    }
}

/// What `PUT /domains/{domain}/manifest` takes: the complete MANIFEST source.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(description = "The full MANIFEST markdown as the editor holds it, \
                        written verbatim: nothing here rebuilds the \
                        frontmatter or stamps provenance.")]
pub struct SaveManifestBody {
    /// The full MANIFEST markdown as the editor holds it.
    #[schema(
        example = "---\ntitle: eng\n---\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions.\n"
    )]
    markdown: String,
}

/// `PUT /domains/{domain}/manifest` - save a domain's MANIFEST markdown
/// verbatim, guarded by the `If-Match` token of the version being replaced.
///
/// Admin, not editor: the spec's domain-management section (slice 3, section
/// 5) places MANIFEST editing among the admin-only domain screens, alongside
/// creating and unregistering a domain - a MANIFEST is what routes an agent
/// into a domain rather than a document inside it, and the Fluid UI gates its
/// own Edit affordance on `canAdminister` to match.
///
/// The same three answers `engrams::save` is held to, because the guard is
/// the same contract: 428 with no `If-Match`, 412 with a stale one (carrying
/// the version the server holds now, so a client can merge), 200 with the new
/// version and its `ETag` once it lands.
#[utoipa::path(
    put,
    path = "/api/v1/domains/{domain}/manifest",
    tag = "domains",
    operation_id = "save_domain_manifest",
    summary = "Save a domain's MANIFEST markdown, guarded by If-Match.",
    description = "The text lands verbatim, frontmatter included, guarded the \
                   same way an engram save is: 428 with no `If-Match`, 412 \
                   when the token is stale (carrying the version the server \
                   holds now), 200 once it lands. A read-only instance \
                   answers 403 ahead of the precondition check, so it is \
                   never 428.",
    params(
        ("domain" = String, Path, description = "The registered domain."),
        (
            "If-Match" = String,
            Header,
            description = "The quoted `ETag` of the version being replaced, \
                           from the manifest read.",
            example = "\"3f8a1c05e2\"",
        ),
    ),
    request_body = SaveManifestBody,
    responses(
        (
            status = 200,
            description = "The manifest as saved, mirroring the GET shape.",
            body = ManifestResponse,
            headers(("etag" = String, description = "The quoted checksum of \
                     the manifest as saved, the token the next save \
                     carries.")),
            example = json!({
                "domain": "eng",
                "markdown": "---\ntitle: eng\n---\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions.\n",
                "checksum": "3f8a1c05e2",
                "sections": {
                    "scope": ["Everything about eng"],
                    "when_to_use": ["Route here for eng questions."],
                    "routing": "when_to_use",
                    "missing": [],
                    "provisioning": null,
                    "tag_aliases": null,
                    "policies": [
                        { "key": "generated_indexes", "declared": null, "effective": "local", "values": ["local", "shared"], "default": "local", "meaning": "Whether the generated folder listings travel with a share.", "changed_by": "owner" },
                        { "key": "sharing", "declared": null, "effective": "proposal", "values": ["proposal", "direct"], "default": "proposal", "meaning": "Whether a share opens a proposal for review or commits straight to the branch.", "changed_by": "owner" }
                    ],
                    "starters": [
                        { "section": "When to Use", "meaning": "Agents pick this domain by these bullets. Without one, nothing routes here.", "example": "## When to Use\n\n- Route here for questions about how our deployment pipeline works" },
                        { "section": "Scope", "meaning": "What belongs in this domain and what does not. Read for routing only when When to Use is empty.", "example": "## Scope\n\n- Infrastructure and deployment, not application code" },
                        { "section": "Provisioning", "meaning": "Folders this domain installs into an AI harness: skills, commands, agents or MCP configs.", "example": "## Provisioning\n\n- skills: skills" },
                        { "section": "Tag Aliases", "meaning": "Spellings that fold into one canonical tag, so a search for either finds both.", "example": "## Tag Aliases\n\n- k8s -> kubernetes" }
                    ]
                }
            }),
        ),
        (
            status = 400,
            description = "`If-Match` carries more than one entity tag: this \
                           surface expects exactly one strong checksum, not a \
                           comma-separated list.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one: an identity with \
                           no account behind it never writes.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "The caller is not an admin, the request did not \
                           echo its CSRF token, this instance is read-only, or \
                           the trusted-header identity names a disabled \
                           account. A read-only instance answers this ahead of \
                           the precondition check, so it is never 428.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or the domain carries no MANIFEST \
                           yet.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 412,
            description = "The `If-Match` token is stale. The body carries \
                           the version the server holds now, so a client can \
                           merge.",
            body = ConflictDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 413,
            description = "The document is over the 10 MiB limit this API \
                           accepts.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 415,
            description = "The body is not `application/json`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "The document carries no frontmatter block, so it \
                           is not a MANIFEST.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 428,
            description = "No `If-Match` arrived. The token comes from the \
                           manifest read.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn save_manifest(
    State(state): State<RestState>,
    identity: Identity,
    headers: HeaderMap,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<SaveManifestBody>,
) -> Result<Response, ApiError> {
    // Admin, not editor: see this function's doc comment for why domain
    // management, MANIFEST editing included, is held to the stronger role.
    identity.require_admin()?;
    // Before the If-Match parse, not after: the same reasoning as
    // `engrams::save`'s own read-only check, repeated here rather than
    // shared, since the handlers are not yet worth abstracting over.
    if state.engine.read_only() {
        return Err(ApiError::forbidden(
            "this instance is read-only; content mutations are disabled",
        ));
    }
    let token = if_match(&headers)?;
    match state
        .engine
        .save_manifest(&domain, &body.markdown, &token)
        .await
    {
        Ok(_) => manifest_response(&domain, body.markdown, StatusCode::OK, None),
        // The same stale-edit translation `engrams::save` makes, repeated
        // rather than shared for the same reason.
        Err(EngineError::Conflict(message)) if message.starts_with(STALE_EDIT) => {
            let current = state.engine.manifest_markdown(&domain).await?;
            let checksum = manifest_checksum(&current);
            Ok(precondition_failed(message, &checksum, current))
        }
        Err(e) => Err(e.into()),
    }
}

/// What `PATCH /domains/{domain}/manifest` takes: registry keys to values.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[schema(
    description = "One or more MANIFEST policy keys to the value each should \
                   hold, for example {\"sharing\": \"direct\"}. Every key is \
                   validated before the first is written."
)]
pub struct SetPoliciesBody(pub std::collections::BTreeMap<String, String>);

/// `PATCH /domains/{domain}/manifest` - set one or more MANIFEST policy keys
/// through the engine's edit path, so disk, index, an unshared local change
/// on a team domain and a draft in a reviewing domain all follow.
///
/// Gates, in order: read-only, an account, domain write, then per key the
/// owner right (the engine's `require_domain_owner`, the rule
/// `set_review_mode` applies) or the admin role for a key the registry marks
/// `admin`. No `If-Match`: the write is one keyed line and the engine's
/// compare-and-write serializes it against a concurrent editor save, whose
/// later whole-document `PUT` still gets its 412.
#[utoipa::path(
    patch,
    path = "/api/v1/domains/{domain}/manifest",
    tag = "domains",
    operation_id = "set_domain_policies",
    summary = "Set one or more MANIFEST policy keys.",
    description = "The domain's owner (or an instance admin) changes the MANIFEST's \
                   configuration keys - `generated_indexes`, `sharing` - without \
                   editing the document: each key becomes one frontmatter line, \
                   every other line stands, and the `generated` block is stamped \
                   as on any edit. On a team domain the MANIFEST becomes an \
                   unshared local change the next share carries; in a domain \
                   that reviews changes the write lands in the caller's own \
                   draft of the MANIFEST and the response says `draft: true`, \
                   the domain's policy changing only when that draft lands. An \
                   unknown key, a value the key does not take, or an empty \
                   object is 422 and writes nothing, even beside a good key.",
    params(("domain" = String, Path, description = "The registered domain.")),
    request_body = SetPoliciesBody,
    responses(
        (
            status = 200,
            description = "The manifest as it now reads for this caller, mirroring the \
                           GET shape, plus `draft: true` when it is the caller's draft.",
            body = ManifestResponse,
            headers(("etag" = String, description = "The quoted checksum of the manifest as it now reads.")),
            example = json!({
                "domain": "kb",
                "markdown": "---\ntitle: kb\nsharing: direct\n---\n\n## Scope\n\n- Everything about kb\n\n## When to Use\n\n- Route here for kb questions.\n",
                "checksum": "3f8a1c05e2",
                "sections": {
                    "scope": ["Everything about kb"],
                    "when_to_use": ["Route here for kb questions."],
                    "routing": "when_to_use",
                    "missing": [],
                    "provisioning": null,
                    "tag_aliases": null,
                    "policies": [
                        { "key": "generated_indexes", "declared": null, "effective": "local", "values": ["local", "shared"], "default": "local", "meaning": "Whether the generated folder listings travel with a share.", "changed_by": "owner" },
                        { "key": "sharing", "declared": "direct", "effective": "direct", "values": ["proposal", "direct"], "default": "proposal", "meaning": "Whether a share opens a proposal for review or commits straight to the branch.", "changed_by": "owner" }
                    ],
                    "starters": [
                        { "section": "When to Use", "meaning": "Agents pick this domain by these bullets. Without one, nothing routes here.", "example": "## When to Use\n\n- Route here for questions about how our deployment pipeline works" },
                        { "section": "Scope", "meaning": "What belongs in this domain and what does not. Read for routing only when When to Use is empty.", "example": "## Scope\n\n- Infrastructure and deployment, not application code" },
                        { "section": "Provisioning", "meaning": "Folders this domain installs into an AI harness: skills, commands, agents or MCP configs.", "example": "## Provisioning\n\n- skills: skills" },
                        { "section": "Tag Aliases", "meaning": "Spellings that fold into one canonical tag, so a search for either finds both.", "example": "## Tag Aliases\n\n- k8s -> kubernetes" }
                    ]
                }
            }),
        ),
        (
            status = 401,
            description = "No identity, or an anonymous one.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 403,
            description = "This instance is read-only (answered ahead of \
                           validation), the caller may write the domain but \
                           is neither its owner nor an admin, the key needs \
                           an admin, or the request did not echo its CSRF \
                           token.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 404,
            description = "No such domain, or none this caller may see.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 415,
            description = "The body is not `application/json`.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
        (
            status = 422,
            description = "An unknown key (the detail names the registry \
                           keys), a value the key does not take (the detail \
                           names the allowed values), or an empty object. \
                           Nothing was written.",
            body = ProblemDetail,
            content_type = "application/problem+json",
        ),
    ),
)]
pub async fn set_domain_policies(
    State(state): State<RestState>,
    identity: Identity,
    ApiPath(domain): ApiPath<String>,
    ApiJson(body): ApiJson<SetPoliciesBody>,
) -> Result<Response, ApiError> {
    super::refuse_read_only(&state)?;
    identity.require_account()?;
    super::require_domain_write(&state, &identity, &domain).await?;
    // The admin gate is this layer's: the engine cannot ask a scope whether
    // it administers the instance. No key needs it today.
    for key in body.0.keys() {
        if policy_registry()
            .iter()
            .any(|spec| spec.key == key && spec.changed_by == crystalline_core::PolicyRole::Admin)
        {
            identity.require_admin()?;
        }
    }
    let changes: Vec<(String, String)> = body.0.into_iter().collect();
    let written = state
        .engine
        .set_manifest_policies(&domain, &changes, &identity.scope())
        .await?;
    let markdown = written["markdown"].as_str().unwrap_or_default().to_string();
    // Said only when it is true: an ordinary domain's answer is the GET shape
    // exactly.
    let extra = (written["draft"] == Value::Bool(true)).then_some(("draft", Value::Bool(true)));
    manifest_response(&domain, markdown, StatusCode::OK, extra)
}

/// The prefix every refused compare-and-swap opens with, wherever the
/// comparison happened. See `engine::stale_edit_message`, the same seam
/// `engrams::STALE_EDIT` classifies on, spelled again here rather than
/// shared across the two modules.
const STALE_EDIT: &str = "stale edit";

/// The manifest response both the GET and the PUT answer with: the domain,
/// the markdown, its checksum, and the same checksum again as a quoted `ETag`
/// header - one shape for a manifest on this surface, so a client that has
/// just saved one holds what the GET route would have given it.
///
/// The sections are read from the markdown on every answer, a save's
/// included, so a client that just saved holds the features of what it saved.
///
/// `extra` is one more top-level key beside the documented shape, which only
/// [`set_domain_policies`] passes: its `draft` says the write landed in the
/// caller's own draft rather than in the domain. A key rather than a field of
/// [`ManifestResponse`], because the other two answers have nothing to say
/// about drafts and would carry it as noise.
fn manifest_response(
    domain: &str,
    markdown: String,
    status: StatusCode,
    extra: Option<(&str, Value)>,
) -> Result<Response, ApiError> {
    let checksum = manifest_checksum(&markdown);
    let etag = HeaderValue::from_str(&format!("\"{checksum}\""))
        .map_err(|_| ApiError::internal("the manifest's checksum is not a usable ETag"))?;
    let sections = ManifestSections::of(&markdown);
    let payload = ManifestResponse {
        domain: domain.to_string(),
        markdown,
        checksum,
        sections,
    };
    let mut resp = match extra {
        None => (status, Json(payload)).into_response(),
        Some((key, value)) => {
            let mut body = serde_json::to_value(payload)
                .map_err(|_| ApiError::internal("the manifest response could not be shaped"))?;
            body[key] = value;
            (status, Json(body)).into_response()
        }
    };
    resp.headers_mut().insert(ETAG, etag);
    Ok(resp)
}

/// The manifest's strong validator: sha256 of the markdown, the same token the
/// engine's save compares. Computed here because the manifest read is a plain
/// string, not an engine read payload that already carries a checksum.
fn manifest_checksum(markdown: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(markdown.as_bytes());
    crystalline_index::hex_lower(&hasher.finalize())
}
