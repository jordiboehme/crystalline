//! The rmcp tool router: the 12 core tools of the v1 MCP surface plus six
//! GitHub collaboration tools whose visibility is gated at runtime.
//!
//! Each tool is a thin wrapper over [`crate::engine::Engine`], which does the
//! real work and is shared with the CLI data commands. Tool descriptions are
//! agent-facing product copy framed around onboarding, teaching, learning and
//! experience. The recommended `type` and `status` value sets are stated in the
//! `write_engram` and `edit_engram` descriptions as guidance; they are never
//! enforced. Every mutating tool requires an explicit domain.
//!
//! The server handshake (`get_info`) hands each connecting agent the live
//! routing block as its `instructions`, rendered from the engine by
//! [`crate::engine::Engine::routing_text`]: the same CRYSTALLINE KNOWLEDGE
//! ROUTING onboarding the CLI `prompt system` emits, minus any workspace
//! scoping, so an agent is routed the moment it connects with no skill or hook
//! required. It re-fetches mid-session through `list_domains` with
//! `include_routing=true`, the same index the instructions carry.
//!
//! In read-only mode (the engine's `read_only` flag) the four content-mutating
//! tools are filtered out of `list_tools` and `get_tool`, so the surface is the
//! eight read tools; the routes stay registered so a client that calls a hidden
//! tool by name reaches the engine's read-only guard and gets a clean error.
//!
//! The six collaboration tools (`configure`, `add_domain`, `share_changes`,
//! `update_domain`, `origin_status`, `resolve_conflict`) are gated the same
//! way, on the engine's live `github.enabled` setting and `read_only` flag
//! rather than a startup snapshot, since `configure` can flip
//! `github.enabled` mid-session: every collaboration tool but `configure`
//! needs `github.enabled`, and `configure`/`add_domain`/`share_changes`/
//! `resolve_conflict` additionally disappear read-only. See `COLLAB_TOOLS`,
//! `COLLAB_WRITE_TOOLS` and `hidden_collab_tool`.
//!
//! rmcp 2.1 supports a server pushing `notifications/tools/list_changed` to
//! a connected client (`Peer::notify_tool_list_changed`, gated behind
//! `ServerCapabilities::enable_tool_list_changed`); `configure` sends one
//! whenever a `set`/`unset` call flips `github.enabled`, so a client that
//! honours the notification refreshes its tool list immediately rather than
//! waiting for its own next poll.

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{Peer, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use serde_json::Value;

use crystalline_remote::RemoteError;

/// The four content-mutating tools. In read-only mode they are hidden from
/// `list_tools` and `get_tool`, while their routes stay registered so a client
/// that calls one by name still reaches the engine guard and gets the read-only
/// error rather than a bare "tool not found".
const WRITE_TOOLS: [&str; 4] = [
    "write_engram",
    "edit_engram",
    "move_engram",
    "delete_engram",
];

/// Whether a tool name is one of the four content-mutating tools.
fn is_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

/// The six GitHub collaboration tools, gated on the engine's live
/// `github.enabled` setting (all but `configure`) and `read_only` flag (see
/// `COLLAB_WRITE_TOOLS`).
const COLLAB_TOOLS: [&str; 6] = [
    "configure",
    "add_domain",
    "share_changes",
    "update_domain",
    "origin_status",
    "resolve_conflict",
];

/// Of the six collaboration tools, the four also hidden in read-only mode:
/// `configure` (settings and this machine's GitHub identity are frozen the
/// same way content is), `add_domain`, `share_changes` and `resolve_conflict`
/// (each writes content, a proposal or config). `update_domain` and
/// `origin_status` stay visible read-only, mirroring their engine-level
/// exemption (a pull is a derived-truth update like sync; status is a pure
/// read).
const COLLAB_WRITE_TOOLS: [&str; 4] = [
    "configure",
    "add_domain",
    "share_changes",
    "resolve_conflict",
];

/// Whether `name` is one of the six collaboration tools.
fn is_collab_tool(name: &str) -> bool {
    COLLAB_TOOLS.contains(&name)
}

/// Whether collaboration tool `name` is hidden given the engine's live
/// `github.enabled` and `read_only` state. Not meaningful for a non-collab
/// tool name; callers check [`is_write_tool`] separately for those. The net
/// matrix: disabled and read-write shows only `configure`; disabled and
/// read-only shows none of the six; enabled and read-write shows all six;
/// enabled and read-only shows `update_domain` and `origin_status` only.
fn hidden_collab_tool(name: &str, github_enabled: bool, read_only: bool) -> bool {
    if read_only && COLLAB_WRITE_TOOLS.contains(&name) {
        return true;
    }
    if !github_enabled && name != "configure" {
        return true;
    }
    false
}

use crate::engine::{ConfigureAction, Engine, EngineError};
use crate::params::*;

/// The shared MCP server: one tool router over one engine. Cheap to clone; the
/// HTTP transport builds one per session.
#[derive(Clone)]
pub struct McpServer {
    engine: Arc<Engine>,
}

impl McpServer {
    /// Build a server around a shared engine.
    pub fn new(engine: Arc<Engine>) -> McpServer {
        McpServer { engine }
    }
}

#[tool_router]
impl McpServer {
    #[tool(
        name = "write_engram",
        description = "Capture a new engram - a unit of knowledge or experience - into a domain so it becomes part of what the agent knows in later sessions. Writes the markdown file (the source of truth) and indexes it. The body is markdown: top-level bullets like '- [decision] we chose X #tag' become observations and '- rel_type [[Target]]' become relations. domain is required so an engram never lands in the wrong place. permalink, status (current), recorded_at (today) and timestamp are filled in for you; valid_from and valid_to are never set, since their absence means always valid. Recommended type values: engram, guide, decision, architecture, runbook, reference. Recommended status values: current, implemented, draft, proposed, idea, poc, deprecated, superseded, archived, legacy - guidance that lets you tell an idea or draft apart from current fact, not a fixed list. Errors if the permalink already exists unless overwrite is true."
    )]
    async fn write_engram(
        &self,
        Parameters(p): Parameters<WriteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .write_engram(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "read_engram",
        description = "Read an engram's full markdown and resolved frontmatter to learn what is already known before acting or writing. Identify it by permalink, domain/permalink, title or a crystalline:// URL; pass domain to disambiguate a bare identifier."
    )]
    async fn read_engram(
        &self,
        Parameters(p): Parameters<ReadParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .read_engram(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "edit_engram",
        description = "Refine an existing engram in place as understanding evolves, rather than rewriting it. Sections are addressed by heading path such as '## API > ### Auth'; replace_section keeps deeper subsections unless include_subsections is set. operation is one of append, prepend, find_replace, replace_section, insert_before_section, insert_after_section. find_replace takes find_text and an optional expected_replacements guard that fails on a count mismatch. Pass expected_checksum (the checksum returned by read_engram) to guard a virtual-domain edit against a change since you last read it: the edit is refused as a conflict if it changed, so re-read and retry; omit it for last-write-wins. The timestamp is refreshed. Recommended status values to reflect a changed lifecycle: current, implemented, draft, proposed, idea, poc, deprecated, superseded, archived, legacy."
    )]
    async fn edit_engram(
        &self,
        Parameters(p): Parameters<EditParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .edit_engram(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "move_engram",
        description = "Re-home an engram to a new path or domain as the knowledge base is reorganized. On a cross-domain move, inbound bare links from other domains are rewritten to the domain-prefixed [[domain:Target]] form so nothing dangles. Set update_links to false to skip that."
    )]
    async fn move_engram(
        &self,
        Parameters(p): Parameters<MoveParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .move_engram(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "delete_engram",
        description = "Remove an engram when its knowledge is retired. Deletes the file and its index rows. Prefer setting status to deprecated or superseded when the history still matters."
    )]
    async fn delete_engram(
        &self,
        Parameters(p): Parameters<DeleteParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .delete_engram(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "search_engrams",
        description = "Search across every registered domain by default (an all-domain sweep) or a chosen few to recall relevant knowledge and experience. Defaults to hybrid lexical-plus-semantic ranking and falls back to plain text when embeddings are not ready. Filter by type, tags, status, arbitrary frontmatter or a recorded-after date; a filter-only search with no query text is allowed. Every hit is labelled with its domain, and a hit inside an observation carries its line."
    )]
    async fn search_engrams(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .search_engrams(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "build_context",
        description = "Assemble the neighbourhood around an anchor engram by following its relations and links, across domains too, to gather related context before a task. The anchor is a crystalline:// URL; a /* suffix globs a permalink prefix. depth is 1 to 3."
    )]
    async fn build_context(
        &self,
        Parameters(p): Parameters<ContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .build_context(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "recent_activity",
        description = "Review what has been captured recently across domains to catch up on new knowledge and experience. Defaults to the last 7 days; timeframe accepts values like 24h, 7d or 2w."
    )]
    async fn recent_activity(
        &self,
        Parameters(p): Parameters<RecentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .recent_activity(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "list_domains",
        description = "List the registered domains with their engram counts to see what the agent has been taught. Set include_routing to also get each domain's When to Use routing bullets from its MANIFEST."
    )]
    async fn list_domains(
        &self,
        Parameters(p): Parameters<ListDomainsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .list_domains(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "browse_domain",
        description = "Browse a domain's engrams by folder to explore how its knowledge is organized. path defaults to the root; depth controls how many folder levels are listed."
    )]
    async fn browse_domain(
        &self,
        Parameters(p): Parameters<BrowseParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .browse_domain(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "validate_engrams",
        description = "Check a domain's engrams against its schema engrams to keep captured knowledge well-formed. Optionally narrow to one engram by identifier or to one type."
    )]
    async fn validate_engrams(
        &self,
        Parameters(p): Parameters<ValidateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .validate_engrams(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "infer_schema",
        description = "Suggest a Picoschema for a type by generalizing over the engrams already captured in a domain, as a starting point for a schema engram. threshold is the frequency at or above which a field is suggested."
    )]
    async fn infer_schema(
        &self,
        Parameters(p): Parameters<InferParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .infer_schema(&p)
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "configure",
        description = "View and adjust Crystalline's settings, like an app's preferences page: call with no arguments to see them, set to change them (for example github.enabled to turn on team collaboration) and connect to link your GitHub account with a short code you confirm in the browser. With a token it accepts a personal access token instead."
    )]
    async fn configure(
        &self,
        Parameters(p): Parameters<ConfigureParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if self.engine.read_only() {
            return Err(to_error(EngineError::ReadOnly));
        }

        if p.token.is_some() || p.connect.is_some() {
            let result = match (p.token.as_deref(), p.connect.as_deref()) {
                (Some(token), _) => {
                    self.engine
                        .connect_with_token(token, p.host.as_deref())
                        .await
                }
                (None, Some("github")) => self.engine.start_device_connect(p.host.as_deref()).await,
                (None, Some(other)) => Err(EngineError::Invalid(format!(
                    "configure connect must be 'github', got '{other}'"
                ))),
                (None, None) => unreachable!("checked above: token or connect is set"),
            };
            return result.map_err(to_error).and_then(ok);
        }

        let before = self.engine.config().github_enabled();
        self.apply_settings(&p).await?;
        let after = self.engine.config().github_enabled();
        if before != after
            && let Err(e) = peer.notify_tool_list_changed().await
        {
            tracing::warn!("failed to send tools/list_changed after configure: {e}");
        }

        self.engine
            .configure_snapshot()
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "add_domain",
        description = "Add a team domain from GitHub: registers the repository as a local domain and downloads its knowledge so the agent can learn from it and share back. repo is owner/name; path points at a subfolder when the domain lives inside a bigger repository."
    )]
    async fn add_domain(
        &self,
        Parameters(p): Parameters<AddDomainParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .origin_add(
                &p.repo,
                p.domain.as_deref(),
                p.path.as_deref(),
                p.branch.as_deref(),
                p.folder.as_deref(),
            )
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "share_changes",
        description = "Share this domain's new knowledge and experience with the team as a proposal they review on GitHub; returns the review URL to hand to the user. Refuses while conflicts are unsettled so the team always reviews a clean proposal."
    )]
    async fn share_changes(
        &self,
        Parameters(p): Parameters<ShareChangesParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .origin_share(&p.domain, p.title.as_deref(), p.description.as_deref())
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "update_domain",
        description = "Learn the team's latest knowledge: pulls what was merged upstream into the domain (or every shared domain), merging cleanly where possible and flagging real conflicts for resolve_conflict."
    )]
    async fn update_domain(
        &self,
        Parameters(p): Parameters<UpdateDomainParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .origin_update(p.domain.as_deref())
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "origin_status",
        description = "Review each shared domain's standing: whether the team has new knowledge to learn, what is waiting to be shared, open and declined proposals and any conflicts to settle."
    )]
    async fn origin_status(
        &self,
        Parameters(p): Parameters<OriginStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .origin_status(p.domain.as_deref())
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "resolve_conflict",
        description = "Settle a flagged conflict by keeping your version (mine), taking the team's version (theirs) or providing merged content. The engram then counts as ordinary local knowledge you can share."
    )]
    async fn resolve_conflict(
        &self,
        Parameters(p): Parameters<ResolveConflictParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let (keep, content): (Option<&str>, Option<&[u8]>) = match p.resolution.as_str() {
            "mine" => (Some("mine"), None),
            "theirs" => (Some("theirs"), None),
            "merged" => {
                let Some(content) = p.content.as_deref() else {
                    return Err(ErrorData::invalid_params(
                        "resolve_conflict requires content when resolution is merged".to_string(),
                        None,
                    ));
                };
                (None, Some(content.as_bytes()))
            }
            other => {
                return Err(ErrorData::invalid_params(
                    format!(
                        "resolve_conflict resolution must be mine, theirs or merged, got '{other}'"
                    ),
                    None,
                ));
            }
        };
        self.engine
            .origin_resolve(&p.domain, &p.path, keep, content)
            .await
            .map_err(to_error)
            .and_then(ok)
    }
}

impl McpServer {
    /// Applies `configure`'s `set` map then `unset` list, one key at a time
    /// through the engine's existing per-key [`ConfigureAction`], stopping at
    /// the first failure. On success every applied key has already taken
    /// effect (and been persisted); on failure the error names which key
    /// failed and which keys before it were already applied, so the caller
    /// never has to guess the resulting state.
    async fn apply_settings(&self, p: &ConfigureParams) -> Result<(), ErrorData> {
        let mut applied: Vec<String> = Vec::new();
        if let Some(set) = &p.set {
            for (key, value) in set {
                match self
                    .engine
                    .configure(&ConfigureAction::Set {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .await
                {
                    Ok(_) => applied.push(key.clone()),
                    Err(e) => return Err(applied_failure(&applied, key, e)),
                }
            }
        }
        if let Some(unset) = &p.unset {
            for key in unset {
                match self
                    .engine
                    .configure(&ConfigureAction::Unset { key: key.clone() })
                    .await
                {
                    Ok(_) => applied.push(key.clone()),
                    Err(e) => return Err(applied_failure(&applied, key, e)),
                }
            }
        }
        Ok(())
    }
}

/// Builds `configure`'s partial-application error: the underlying error's
/// class (invalid params vs internal) is kept, only the message is enriched
/// with which keys already applied and which one failed.
fn applied_failure(applied: &[String], failed_key: &str, e: EngineError) -> ErrorData {
    let base = to_error(e);
    let message = if applied.is_empty() {
        format!("failed to apply '{failed_key}': {}", base.message)
    } else {
        format!(
            "applied [{}]; failed to apply '{failed_key}': {}",
            applied.join(", "),
            base.message
        )
    };
    ErrorData::new(base.code, message, base.data)
}

#[tool_handler]
impl ServerHandler for McpServer {
    /// The server handshake: hand the connecting agent the live routing block
    /// as its `instructions`. rmcp calls `get_info` once per connection at
    /// initialize, so [`Engine::routing_text`] renders the currently registered
    /// domains (a domain added since startup shows up on the next connection)
    /// and follows the engine's read-only mode, read-write and read-only intros
    /// alike. The daemon and the embedded stdio stack refresh the
    /// virtual-domain routing cache just before this runs, so the sync render
    /// reads a current cache and never blocks on the store.
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some(self.engine.routing_text());
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build();
        info
    }

    /// List the exposed tools. In read-only mode the four content-mutating
    /// tools are filtered out so they are absent from `tools/list`, while their
    /// routes stay registered for the call-by-name guard (see `WRITE_TOOLS`).
    /// The six collaboration tools are filtered the same way against the
    /// engine's live `github.enabled` and `read_only` state (see
    /// `hidden_collab_tool`), consulted fresh on every call rather than
    /// cached, since `configure` can flip `github.enabled` mid-session.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let read_only = self.engine.read_only();
        let github_enabled = self.engine.config().github_enabled();
        let mut tools = Self::tool_router().list_all();
        tools.retain(|t| {
            if is_write_tool(&t.name) && read_only {
                return false;
            }
            if is_collab_tool(&t.name) && hidden_collab_tool(&t.name, github_enabled, read_only) {
                return false;
            }
            true
        });
        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    /// Resolve a tool definition by name, hiding the content-mutating and
    /// gated collaboration tools the same way `list_tools` does, so a hidden
    /// tool never surfaces through `get_tool` either.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        let read_only = self.engine.read_only();
        if is_write_tool(name) && read_only {
            return None;
        }
        if is_collab_tool(name) {
            let github_enabled = self.engine.config().github_enabled();
            if hidden_collab_tool(name, github_enabled, read_only) {
                return None;
            }
        }
        Self::tool_router().get(name).cloned()
    }
}

/// Wrap an engine value as a successful tool result. The compact JSON is the
/// single text content block; callers that need structured data re-parse it.
fn ok(value: Value) -> Result<CallToolResult, ErrorData> {
    let text = serde_json::to_string(&value)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
}

/// Map an engine error to an rmcp tool error with an actionable message.
fn to_error(e: EngineError) -> ErrorData {
    match e {
        EngineError::UnknownDomain { .. }
        | EngineError::NotFound(_)
        | EngineError::Ambiguous(_)
        | EngineError::Conflict(_)
        | EngineError::Invalid(_)
        | EngineError::ReadOnly
        | EngineError::EnvTokenConnect => ErrorData::invalid_params(e.to_string(), None),
        EngineError::Remote(remote) => remote_to_error(remote),
        EngineError::Io { .. } | EngineError::Internal(_) => {
            ErrorData::internal_error(e.to_string(), None)
        }
    }
}

/// Map a GitHub collaboration error to an rmcp tool error, splitting by
/// whether the caller is at fault. Transient or environmental variants -
/// offline, rate limited, an expired connection or a still-pending sign-in,
/// plus an unexpected upstream answer, a filesystem or credential-store
/// failure and a rewritten repository history that re-baselines on its own -
/// are never the caller's mistake, so they map to the internal/server error
/// class rather than `invalid_params`; the message (already actionable
/// product copy, see `crystalline_remote::error`) is carried verbatim
/// either way. Genuine input problems - collaboration turned off, no
/// connection yet, an unreachable repository, a repository or subpath with
/// no domain, unresolved conflicts blocking a share, or a proposal or
/// conflict path that does not exist - stay `invalid_params`-shaped. This
/// match is exhaustive over `RemoteError` so a new variant must be
/// classified here rather than silently defaulting.
fn remote_to_error(e: RemoteError) -> ErrorData {
    let message = e.to_string();
    match e {
        RemoteError::Offline
        | RemoteError::RateLimited { .. }
        | RemoteError::AuthExpired
        | RemoteError::AuthPending
        | RemoteError::Api { .. }
        | RemoteError::Io(_)
        | RemoteError::State(_)
        | RemoteError::Credential { .. }
        | RemoteError::BaseUnavailable => ErrorData::internal_error(message, None),
        RemoteError::NotEnabled
        | RemoteError::NotConnected
        | RemoteError::RepoNotFound { .. }
        | RemoteError::NotADomain { .. }
        | RemoteError::ConflictsPending { .. }
        | RemoteError::ProposalNotFound { .. }
        | RemoteError::ConflictNotFound { .. } => ErrorData::invalid_params(message, None),
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::ErrorCode;

    use super::*;

    #[test]
    fn transient_remote_errors_map_to_the_internal_error_class() {
        let cases = [
            RemoteError::Offline,
            RemoteError::RateLimited { reset: None },
            RemoteError::AuthExpired,
            RemoteError::AuthPending,
            RemoteError::Api {
                status: 502,
                message: "bad gateway".to_string(),
            },
            RemoteError::State("corrupt".to_string()),
            RemoteError::Credential {
                detail: "locked".to_string(),
            },
            RemoteError::BaseUnavailable,
        ];
        for e in cases {
            let message = e.to_string();
            let err = remote_to_error(e);
            assert_eq!(
                err.code,
                ErrorCode::INTERNAL_ERROR,
                "{message} should not read as a client mistake"
            );
            assert_eq!(err.message, message, "the actionable message is verbatim");
        }
    }

    #[test]
    fn genuine_input_remote_errors_map_to_invalid_params() {
        let cases = [
            RemoteError::NotEnabled,
            RemoteError::NotConnected,
            RemoteError::RepoNotFound {
                repo: "acme/brand-knowledge".to_string(),
            },
            RemoteError::NotADomain {
                repo: "acme/brand-knowledge".to_string(),
                path: None,
            },
            RemoteError::ConflictsPending { count: 2 },
            RemoteError::ProposalNotFound { number: 7 },
            RemoteError::ConflictNotFound {
                path: "notes/a.md".to_string(),
                open: vec![],
            },
        ];
        for e in cases {
            let message = e.to_string();
            let err = remote_to_error(e);
            assert_eq!(err.code, ErrorCode::INVALID_PARAMS, "{message}");
            assert_eq!(err.message, message);
        }
    }

    #[test]
    fn to_error_routes_remote_through_the_same_class_split() {
        let err = to_error(EngineError::Remote(RemoteError::NotEnabled));
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);

        let err = to_error(EngineError::Remote(RemoteError::Offline));
        assert_eq!(err.code, ErrorCode::INTERNAL_ERROR);
    }

    #[test]
    fn is_collab_tool_recognizes_exactly_the_six() {
        for name in COLLAB_TOOLS {
            assert!(is_collab_tool(name), "{name}");
        }
        assert!(!is_collab_tool("write_engram"));
        assert!(!is_collab_tool("search_engrams"));
    }

    #[test]
    fn hidden_collab_tool_matches_the_locked_gating_matrix() {
        // disabled + read-write: only configure of the six is visible.
        assert!(!hidden_collab_tool("configure", false, false));
        for name in [
            "add_domain",
            "share_changes",
            "update_domain",
            "origin_status",
            "resolve_conflict",
        ] {
            assert!(hidden_collab_tool(name, false, false), "{name}");
        }

        // disabled + read-only: none of the six are visible.
        for name in COLLAB_TOOLS {
            assert!(hidden_collab_tool(name, false, true), "{name}");
        }

        // enabled + read-write: all six are visible.
        for name in COLLAB_TOOLS {
            assert!(!hidden_collab_tool(name, true, false), "{name}");
        }

        // enabled + read-only: only update_domain and origin_status are visible.
        for name in ["update_domain", "origin_status"] {
            assert!(!hidden_collab_tool(name, true, true), "{name}");
        }
        for name in [
            "configure",
            "add_domain",
            "share_changes",
            "resolve_conflict",
        ] {
            assert!(hidden_collab_tool(name, true, true), "{name}");
        }
    }
}
