//! The rmcp tool router: the core tools of the v1 MCP surface plus the gated
//! collaboration tools, visible only when the engine's live settings allow.
//!
//! Each tool is a thin wrapper over [`crate::engine::Engine`], which does the
//! real work and is shared with the CLI data commands. Tool descriptions are
//! agent-facing product copy framed around onboarding, teaching, learning and
//! experience. The recommended `type` and `status` value sets are stated once,
//! in the `write_engram` description, as guidance that is never enforced;
//! `edit_engram` points back to it rather than repeating the list. Every
//! mutating tool requires an explicit domain.
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
//! ten read tools; the routes stay registered so a client that calls a hidden
//! tool by name reaches the engine's read-only guard and gets a clean error.
//!
//! The collaboration tools (`configure`, `add_domain`, `share_changes`,
//! `update_domain`, `origin_status`, `resolve_conflict`) are gated the same
//! way, on the engine's live `github.enabled` setting and `read_only` flag
//! rather than a startup snapshot, since `configure` can flip
//! `github.enabled` mid-session: every collaboration tool but `configure`
//! needs `github.enabled`, and `configure`/`add_domain`/`share_changes`/
//! `resolve_conflict` additionally disappear read-only. See `COLLAB_TOOLS`,
//! `COLLAB_WRITE_TOOLS` and `hidden_collab_tool`.
//!
//! One more tool, `provision`, is gated a third way: hidden whenever no
//! registered domain's MANIFEST declares a `## Provisioning` section (see
//! [`Engine::provisioning_declared`]) or the instance is read-only, so an
//! install with nothing to provision never carries the tool's context cost.
//! Its route stays registered like every other hidden tool, so a call by
//! name still reaches the engine: `status` answers for real even with no
//! declaring domain, and a mutating action still hits the read-only guard.
//!
//! The shipped agent skills are served here too, so a remote client that never
//! runs `crystalline install` can still learn how to use Crystalline well:
//! the `skills` tool (an index with no arguments, one skill's full `SKILL.md`
//! with `name`), five `skill://<name>/SKILL.md` resources and two prompts,
//! `onboarding` (the live routing block) and `connector` (the static snippet
//! that teaches a client to onboard itself). The whole surface shares one
//! gate, the live `skills.serve` setting; when it hides the surface the tool,
//! the resource list and the prompt list are empty while direct reads keep
//! answering, the same hidden-not-disabled doctrine the tool gates follow.
//!
//! That gate is tri-state and its default, `auto`, is decided per connection:
//! a stdio client whose `initialize` name maps to a harness this machine's
//! install receipt onboarded with session hooks already carries those skills
//! as files and gets the routing block from its own hook, so it is served
//! neither the surface nor the full instructions block (see
//! `hidden_skills_surface`, `minimal_instructions` and the `initialize`
//! override, which is the only place a server can see who is connecting).
//! `true` and `false` force the old always and never behaviour. An HTTP
//! session is never suppressed: a remote client is exactly who the served
//! surface is for.
//!
//! The resource shape follows the converging skills-over-MCP proposal
//! without advertising its extension id, which is not ratified yet. The
//! prompts are declared with rmcp's `#[prompt_router]`/`#[prompt]` macros but
//! `list_prompts` and `get_prompt` are hand-written, since
//! `#[prompt_handler]` replaces any `list_prompts` in its impl block and the
//! gate needs one it can empty.
//!
//! rmcp 2.1 supports a server pushing `notifications/tools/list_changed` to
//! a connected client (`Peer::notify_tool_list_changed`, gated behind
//! `ServerCapabilities::enable_tool_list_changed`); `configure` sends one
//! whenever a `set`/`unset` call flips `github.enabled`, and `add_domain`/
//! `update_domain` send one whenever they flip whether any domain declares
//! provisioning, so a client that honours the notification refreshes its
//! tool list immediately rather than waiting for its own next poll. A
//! `skills.serve` flip moves three lists at once, so `configure` sends the
//! prompt and resource list-changed notifications alongside the tool one.
//!
//! Every tool also advertises MCP tool annotations: a display `title` plus the
//! readOnly/destructive/idempotent/openWorld hints, so a client can tune its
//! confirmation UX and batch the read-only calls. The hints are advisory only;
//! enforcement stays the runtime gating (`WRITE_TOOLS`, `hidden_collab_tool`)
//! and the engine guards. Two calls are deliberate: `write_engram` advertises
//! non-destructive because its default behaviour is additive (it errors on an
//! existing permalink unless `overwrite`), and `open_world` is true only for
//! the tools that talk to GitHub - `configure` through its connect flow,
//! `add_domain` through team mode, `share_changes`, `update_domain` and
//! `origin_status`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rmcp::handler::server::prompt::PromptContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, GetPromptRequestParams, GetPromptResult,
    Implementation, InitializeRequestParams, InitializeResult, ListPromptsResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParams, ProgressNotificationParam,
    PromptMessage, ProtocolVersion, ReadResourceRequestParams, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{
    Peer, RoleServer, ServerHandler, prompt, prompt_router, tool, tool_handler, tool_router,
};
use serde_json::{Value, json};

use crystalline_core::SKILL_ASSETS;
use crystalline_remote::RemoteError;

/// The tools hidden in read-only mode: the four content-mutating engram tools
/// plus `add_domain`, which creates a domain (writing config, and files for a
/// local domain). In read-only mode they are hidden from `list_tools` and
/// `get_tool`, while their routes stay registered so a client that calls one by
/// name still reaches the engine guard and gets the read-only error rather than
/// a bare "tool not found".
const WRITE_TOOLS: [&str; 5] = [
    "write_engram",
    "edit_engram",
    "move_engram",
    "delete_engram",
    "add_domain",
];

/// Whether a tool name is one of the write-gated tools (hidden in read-only
/// mode).
fn is_write_tool(name: &str) -> bool {
    WRITE_TOOLS.contains(&name)
}

/// The five GitHub collaboration tools, gated on the engine's live
/// `github.enabled` setting (all but `configure`) and `read_only` flag (see
/// `COLLAB_WRITE_TOOLS`). `add_domain` is not among them: it creates domains of
/// every kind, so it is a write-gated tool (see `WRITE_TOOLS`), and only its
/// team-domain branch needs `github.enabled`, enforced in the engine.
const COLLAB_TOOLS: [&str; 5] = [
    "configure",
    "share_changes",
    "update_domain",
    "origin_status",
    "resolve_conflict",
];

/// Of the five collaboration tools, the three also hidden in read-only mode:
/// `configure` (settings and this machine's GitHub identity are frozen the
/// same way content is), `share_changes` and `resolve_conflict` (each writes a
/// proposal or config). `update_domain` and `origin_status` stay visible
/// read-only, mirroring their engine-level exemption (a pull is a derived-truth
/// update like sync; status is a pure read).
const COLLAB_WRITE_TOOLS: [&str; 3] = ["configure", "share_changes", "resolve_conflict"];

/// Appended to the initialize instructions while TOON responses are active,
/// so a client model reads list results as structured data rather than prose.
const TOON_INSTRUCTIONS_NOTE: &str = "\n\nList-shaped tool results (search hits, activity, listings and status reports) arrive TOON-encoded rather than as JSON: indentation nests objects, `name[N]{field1,field2}:` heads a uniform array with one comma-separated row per record and a tags cell joins its values with commas. Read them as data with exactly those fields.";

/// Whether `name` is one of the five collaboration tools.
fn is_collab_tool(name: &str) -> bool {
    COLLAB_TOOLS.contains(&name)
}

/// Whether the `provision` tool is hidden given the engine's live read-only
/// state and whether any registered domain currently declares a
/// `## Provisioning` section. A fresh install with nothing to provision never
/// carries the tool's context cost; the route stays registered regardless
/// (see `list_tools` and `get_tool`), so a call by name still reaches the
/// engine either way.
fn hidden_provision_tool(read_only: bool, provisioning_declared: bool) -> bool {
    read_only || !provisioning_declared
}

/// The MIME type every shipped skill is served with, as a resource and in the
/// `skills` tool's full read: a `SKILL.md` is plain markdown.
const SKILL_MIME_TYPE: &str = "text/markdown";

/// The resource uri one shipped skill is served under. The shape follows the
/// converging skills-over-MCP proposal (`skill://<name>/SKILL.md`) without
/// advertising its extension id, which is not ratified yet: a client that
/// learns the shape reads the same bytes either way.
fn skill_uri(name: &str) -> String {
    format!("skill://{name}/SKILL.md")
}

/// The shipped skill a resource uri names, or `None` when the uri is not one
/// of the five this server serves.
fn skill_for_uri(uri: &str) -> Option<&'static crystalline_core::SkillAsset> {
    let name = uri.strip_prefix("skill://")?.strip_suffix("/SKILL.md")?;
    crystalline_core::skill(name)
}

/// The five shipped skill names, comma separated, for an error that names
/// what the caller could have asked for instead.
fn skill_names() -> String {
    SKILL_ASSETS
        .iter()
        .map(|s| s.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The five skill resource uris, comma separated, for the same reason.
fn skill_uris() -> String {
    SKILL_ASSETS
        .iter()
        .map(|s| skill_uri(s.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether the whole skill-serving surface (the `skills` tool, the `skill://`
/// resources and the two prompts) is hidden for one connection.
///
/// `skills_serve` is the engine's live setting, read fresh on every call
/// rather than cached, since `configure` can flip it mid-session.
/// `receipt_matched` is the connection fact [`McpServer::initialize`] decided
/// once at handshake time: a local (stdio) client this machine's install
/// receipt knows as an onboarded harness with session hooks wired. The three
/// rows are exactly the setting's value set:
///
/// - `true`: never hidden, whoever connects.
/// - `false`: always hidden, whoever connects.
/// - `auto`: hidden for a receipt-matched connection only, since that client
///   already carries the same skills as files.
///
/// Hidden means hidden, not disabled: the lists come back empty while the
/// tool, the resources and the prompts all keep answering a direct call.
/// Read-only mode is not part of it either: reading a skill is a read.
fn hidden_skills_surface(skills_serve: SkillsServe, receipt_matched: bool) -> bool {
    match skills_serve {
        SkillsServe::Always => false,
        SkillsServe::Never => true,
        SkillsServe::Auto => receipt_matched,
    }
}

/// Whether one connection gets the minimal `instructions` block instead of the
/// full routing block: only under `auto`, and only for a receipt-matched
/// client, whose own session hook has already delivered the full block.
///
/// `false` deliberately does not shrink the instructions. That setting gates
/// serving skills, not onboarding: an operator who turns the skill surface off
/// still wants a connecting agent to learn which domains exist.
fn minimal_instructions(skills_serve: SkillsServe, receipt_matched: bool) -> bool {
    skills_serve == SkillsServe::Auto && receipt_matched
}

/// Whether the client that sent `client_name` in its `initialize` handshake is
/// a harness this machine has onboarded with session hooks wired, given
/// `hooked` (the receipt's hooks-installed harnesses). An unrecognized client
/// name never matches, and a harness whose install skipped hooks is not in
/// `hooked`, so it does not match either.
fn receipt_matches_client(client_name: &str, hooked: &[crystalline_core::HarnessKind]) -> bool {
    match crystalline_core::HarnessKind::from_mcp_client_name(client_name) {
        Some(kind) => hooked.contains(&kind),
        None => false,
    }
}

/// Whether collaboration tool `name` is hidden given the engine's live
/// `github.enabled` and `read_only` state. Not meaningful for a non-collab
/// tool name; callers check [`is_write_tool`] separately for those. The net
/// matrix: disabled and read-write shows only `configure`; disabled and
/// read-only shows none of the five; enabled and read-write shows all five;
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

use crystalline_core::config::{ResponseFormat, SkillsServe};

use crate::engine::{ConfigureAction, Engine, EngineError, ProvisionAction};
use crate::params::*;

/// The connected client's identity in the OKF agent form `name/version`, read
/// from the initialize handshake rmcp keeps on the peer.
///
/// The peer is per connection and the request context carries it into every
/// tool call, so a write records who actually asked for it even when several
/// HTTP sessions share one engine. `None` when the handshake carried no usable
/// name; [`Engine::actor`] then falls back. A version of `0.0.0` (rmcp's
/// stand-in for a client that sent none) is dropped rather than recorded.
fn client_actor(ctx: &RequestContext<RoleServer>) -> Option<String> {
    let info = ctx.peer.peer_info()?;
    let name = info.client_info.name.trim();
    if name.is_empty() {
        return None;
    }
    let version = info.client_info.version.trim();
    if version.is_empty() || version == "0.0.0" {
        return Some(name.to_string());
    }
    Some(format!("{name}/{version}"))
}

/// Which transport a server instance serves, the one distinction the `auto`
/// value of `skills.serve` turns on.
///
/// A stdio connection is by construction same-machine: the client is a process
/// this machine's harness started, so this machine's install receipt is
/// authoritative about what that client already has on disk. An HTTP session
/// says nothing of the kind, so it is never suppressed - a remote client is
/// exactly the case the served skill surface exists for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Served over stdio: the `crystalline mcp` bridge, its daemon relay and
    /// the embedded in-process stack.
    Stdio,
    /// Served over the streamable HTTP transport.
    Http,
}

/// The MCP server for one connection: one tool router over one shared engine.
/// Cheap to clone; every serving path builds one per connection (the daemon
/// per accepted `mcp` socket, the HTTP transport per session, the stdio bridge
/// once for its single session), which is what lets it hold the per-connection
/// handshake decision below.
#[derive(Clone)]
pub struct McpServer {
    engine: Arc<Engine>,
    transport: Transport,
    /// Where to read this machine's install receipt. `None` when the state
    /// directory could not be resolved at all, which reads as "no receipt".
    install_receipt: Option<PathBuf>,
    /// Whether the client that completed this connection's `initialize` is a
    /// locally installed harness with session hooks wired. Decided once in
    /// [`McpServer::initialize`] and read by every gate afterwards, so the
    /// receipt is read once per connection rather than once per list call.
    /// Shared through the clone rmcp keeps, hence the atomic.
    receipt_matched: Arc<AtomicBool>,
}

impl McpServer {
    /// Build a server around a shared engine for a stdio connection.
    pub fn new(engine: Arc<Engine>) -> McpServer {
        McpServer::with_transport(engine, Transport::Stdio)
    }

    /// Build a server around a shared engine for one HTTP session.
    pub fn new_http(engine: Arc<Engine>) -> McpServer {
        McpServer::with_transport(engine, Transport::Http)
    }

    fn with_transport(engine: Arc<Engine>, transport: Transport) -> McpServer {
        McpServer {
            engine,
            transport,
            install_receipt: crystalline_core::provision::install_receipt_path().ok(),
            receipt_matched: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Point this server at an explicit install receipt instead of the one
    /// under this machine's state directory. Tests use it to exercise the
    /// `auto` matching without touching the developer's real receipt.
    pub fn with_install_receipt(mut self, path: PathBuf) -> McpServer {
        self.install_receipt = Some(path);
        self
    }

    /// Whether this connection's client matched the install receipt, the fact
    /// [`hidden_skills_surface`] and [`minimal_instructions`] gate on. Always
    /// `false` before `initialize` has run and always `false` on HTTP.
    fn receipt_matched(&self) -> bool {
        self.receipt_matched.load(Ordering::Relaxed)
    }
}

#[tool_router]
impl McpServer {
    #[tool(
        name = "write_engram",
        title = "Capture engram",
        description = "Capture a new engram - a unit of knowledge - into a domain. Writes the markdown file and indexes it. Body bullets: '- [decision] we chose X #tag' become observations, '- rel_type [[Target]]' become relations. domain is required so an engram never lands in the wrong place. Pass folder to file the engram under a topic prefix: reuse the domain's existing layout (browse_domain shows it), start a subfolder when a topic cluster is forming and keep singletons at the root; the folder path becomes the permalink prefix build_context globs as crystalline://domain/folder/*. permalink, status, recorded_at and generated (who wrote it and when) are filled in; valid_from/valid_to are never auto-set - absence means always valid; to bound validity pass them inside metadata as plain ISO dates (YYYY-MM-DD). Any other date format is rejected; a sentinel far-future valid_to and an explicit null are dropped, since absence already means valid forever. Recommended type values: engram, guide, decision, architecture, runbook, reference. Recommended status values (guidance, not enforced): stable, implemented, draft, proposed, idea, poc, deprecated, superseded, archived, legacy. stable is the default and the word for knowledge that holds now; current is the legacy alias for the same state, and a status filter on either word matches engrams carrying either. Of those, deprecated, superseded, archived and legacy are the recognized retirement set: a status inside it softly fades in search ranking, any other value ranks at full strength. Errors if the permalink exists unless overwrite is true, and refuses a title that would file the engram as the reserved index.md or log.md (Crystalline generates the folder index itself). The vocabulary tool lists tags already in use; reuse one before coining a new tag. Set an optional numeric salience metadata key (0-10) to mark exceptionally valuable knowledge; salient engrams are lifted in hybrid search ranking. Raise it later to elevate an engram that proved load-bearing.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn write_engram(
        &self,
        Parameters(p): Parameters<WriteParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .write_engram_as(&p, client_actor(&ctx).as_deref())
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "read_engram",
        title = "Read engram",
        description = "Read an engram's full markdown and resolved frontmatter to learn what is already known before acting or writing. Identify it by bare permalink, title or a crystalline:// URL; pass domain to disambiguate. An identifier without crystalline:// is domain-relative: 'onboarding/setup', never 'mydomain/onboarding/setup'. The response flags whether each relation and prose link resolves, summarizes what links back and names a build_context anchor for exploring nearby knowledge.",
        annotations(read_only_hint = true, open_world_hint = false)
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
        title = "Edit engram",
        description = "Refine an existing engram in place as understanding evolves. Sections are addressed by heading path such as '## API > ### Auth'; replace_section keeps deeper subsections unless include_subsections is set. operation is one of append, prepend, find_replace, replace_section, insert_before_section, insert_after_section. find_replace takes find_text and an optional expected_replacements guard that fails on a count mismatch. Pass expected_checksum (from read_engram) to guard a virtual-domain edit against a change since your read: a conflict is refused if it changed, so re-read and retry; omit it for last-write-wins. The generated provenance block is refreshed with who edited it and when. Status values to reflect a changed lifecycle (recommended values: see write_engram). Temporal frontmatter fields (recorded_at, valid_from, valid_to, source_date, stale_after, plus the legacy last_verified and review_after spellings) must stay plain ISO dates (YYYY-MM-DD): an edit that leaves one malformed is rejected and a sentinel far-future valid_to or an explicit null is dropped, except recorded_at which is required and cannot be nulled.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn edit_engram(
        &self,
        Parameters(p): Parameters<EditParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .edit_engram_as(&p, client_actor(&ctx).as_deref())
            .await
            .map_err(to_error)
            .and_then(ok)
    }

    #[tool(
        name = "move_engram",
        title = "Move engram",
        description = "Re-home an engram to a new path or domain as the knowledge base is reorganized. The destination may stay inside the same domain: re-filing an engram into a topic subfolder as a cluster forms is a normal move. On a cross-domain move, inbound bare links from other domains are rewritten to the domain-prefixed [[domain:Target]] form so nothing dangles. Set update_links to false to skip that. A destination filename of index.md or log.md is refused: both names are reserved for the generated directory index and log.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
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
        title = "Delete engram",
        description = "Remove an engram when its knowledge is retired. Deletes the file and its index rows. Prefer setting status to deprecated or superseded when the history still matters.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
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
        title = "Search engrams",
        description = "Search across every registered domain by default (an all-domain sweep) or a chosen few to recall relevant knowledge and experience. Defaults to hybrid lexical-plus-semantic ranking and falls back to plain text when embeddings are not ready. Filter by type, tags, status, arbitrary frontmatter or a recorded-after date; a filter-only search with no query text is allowed. Every hit is labelled with its domain, and a hit inside an observation carries its line. A hit's snippet is a short window around the match, never the whole engram: read_engram returns the full content, so read before citing or summarizing what a hit only previews. The result reports total, page, limit and count; when count is below total, request the next page to see the rest. A tags filter also matches through a domain's tag aliases (the MANIFEST `## Tag Aliases` section), so a merged old tag name still finds its engrams. A status filter on stable or current matches both, since they are one state under two spellings; any other status matches exactly. Hybrid ranking adds a small salience prior, so an engram marked salient at write time ranks above equally relevant unmarked ones without ever excluding a result. Engrams whose status is deprecated, superseded, archived or legacy are softly faded in ranking (the search.retired_weight setting, default 0.6, 1.0 disables), reordered but never excluded.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn search_engrams(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .search_engrams(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "build_context",
        title = "Build context",
        description = "Assemble the neighbourhood around an anchor engram by following its relations and links, across domains too, to gather related context before a task. Related engrams come back ranked by how strongly they connect to the anchor, salience-aware and status-aware (retired statuses rank lower), and max_related keeps the top-ranked. The anchor is a crystalline:// URL; a /* suffix globs a permalink prefix. depth is 1 to 3.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn build_context(
        &self,
        Parameters(p): Parameters<ContextParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .build_context(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "recent_activity",
        title = "Recent activity",
        description = "Review what has been captured recently across domains to catch up on new knowledge and experience. Defaults to the last 7 days; timeframe accepts values like 24h, 7d or 2w.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn recent_activity(
        &self,
        Parameters(p): Parameters<RecentParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .recent_activity(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "list_domains",
        title = "List domains",
        description = "List the registered domains with their engram counts to see what the agent has been taught. If no CRYSTALLINE KNOWLEDGE ROUTING block reached you this session, call this at session start with include_routing=true: it returns each domain's When to Use routing bullets plus the behavior rules for this server's tools; follow them and route searches through those domains before answering from memory. The same call re-fetches the index mid-session.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_domains(
        &self,
        Parameters(p): Parameters<ListDomainsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .list_domains(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "browse_domain",
        title = "Browse domain",
        description = "Browse a domain's engrams by folder to explore how its knowledge is organized. path defaults to the root; depth controls how many folder levels are listed.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn browse_domain(
        &self,
        Parameters(p): Parameters<BrowseParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .browse_domain(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "validate_engrams",
        title = "Validate engrams",
        description = "Check a domain's engrams against its schema engrams to keep captured knowledge well-formed. Optionally narrow to one engram by identifier or to one type. Also runs the temporal checks so malformed dates, inverted validity windows, sentinel far-future dates and malformed generated or verified provenance entries are reported. Set drift to also report observation categories and relation types that drift from the schema - in use but undeclared or declared but unused.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn validate_engrams(
        &self,
        Parameters(p): Parameters<ValidateParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .validate_engrams(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "infer_schema",
        title = "Infer schema",
        description = "Suggest a Picoschema for a type by generalizing over the engrams already captured in a domain, as a starting point for a schema engram. threshold is the frequency at or above which a field is suggested.",
        annotations(read_only_hint = true, open_world_hint = false)
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
        name = "vocabulary",
        title = "Vocabulary in use",
        description = "List the vocabulary in use: tags with engram and observation usage counts, observation categories with counts and relation types with counts, for one domain or across all domains. Check it before inventing a new tag or category so existing terms are reused instead of multiplied. Near-duplicate tag clusters are reported so they can be merged. Tag aliases recorded in a MANIFEST are listed too and clusters an alias already explains are not reported.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn vocabulary(
        &self,
        Parameters(p): Parameters<VocabularyParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .vocabulary(&p)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "configure",
        title = "Configure Crystalline",
        description = "View and adjust Crystalline's settings, like an app's preferences page: call with no arguments to see them, set to change them (for example github.enabled to turn on team collaboration) and connect to link your GitHub account with a short code you confirm in the browser. With a token it accepts a personal access token instead. Connecting works before or after enabling; only team domains need github.enabled turned on.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = true
        )
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

        let before = self.engine.github_enabled();
        // Compare the surface this connection actually sees, not the raw
        // setting: under `auto` the same value can mean hidden for a
        // receipt-matched client and visible for everyone else, so a flip
        // between `auto` and `true` moves nothing for a client that was never
        // matched and must not claim otherwise.
        let matched = self.receipt_matched();
        let skills_before = hidden_skills_surface(self.engine.skills_serve(), matched);
        self.apply_settings(&p).await?;
        let after = self.engine.github_enabled();
        let skills_after = hidden_skills_surface(self.engine.skills_serve(), matched);
        // A `skills.serve` flip moves three lists at once (the `skills` tool,
        // the `skill://` resources and the two prompts), a `github.enabled`
        // flip only the tool list; one call can do both, and the tool
        // notification is sent once either way.
        let skills_flipped = skills_before != skills_after;
        if (before != after || skills_flipped)
            && let Err(e) = peer.notify_tool_list_changed().await
        {
            tracing::warn!("failed to send tools/list_changed after configure: {e}");
        }
        if skills_flipped {
            if let Err(e) = peer.notify_prompt_list_changed().await {
                tracing::warn!("failed to send prompts/list_changed after configure: {e}");
            }
            if let Err(e) = peer.notify_resource_list_changed().await {
                tracing::warn!("failed to send resources/list_changed after configure: {e}");
            }
        }

        self.engine
            .configure_snapshot()
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "add_domain",
        title = "Add domain",
        description = "Create or connect a domain to store engrams in - the way to give the agent somewhere to capture knowledge, so it works even on an instance with no domains yet. Three modes follow the arguments: a local domain of markdown files on disk (pass folder, or just domain to use the default root at <domains_root>/<domain>) that is created with a starter MANIFEST when new and adopted in place when it already holds engrams; a virtual database-backed domain with no files (virtual: true with a domain name); or a GitHub team domain that downloads shared knowledge to learn from and share back (repo is owner/name, needs GitHub enabled via configure). repo and virtual are mutually exclusive. Available whenever the instance is writable; only the team mode needs GitHub turned on. Connecting a repository this domain is already connected to is safe and simply reports the connected state. Connecting a repository reports progress while it downloads and registers the knowledge, then keeps embedding it for search in the background after the call returns.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn add_domain(
        &self,
        Parameters(p): Parameters<AddDomainParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if p.repo.is_some() && p.is_virtual {
            return Err(to_error(EngineError::Invalid(
                "add_domain: repo and virtual are mutually exclusive; a team domain is file-backed"
                    .to_string(),
            )));
        }

        // When the client sent a progress token, forward stage boundaries as
        // MCP progress notifications so its request timeout stays alive during
        // the download; a channel plus one forwarder task keeps them ordered.
        let progress = ctx.meta.get_progress_token().map(|token| {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, u64, String)>();
            let peer = ctx.peer.clone();
            tokio::spawn(async move {
                while let Some((step, total, message)) = rx.recv().await {
                    let _ = peer
                        .notify_progress(
                            ProgressNotificationParam::new(token.clone(), step as f64)
                                .with_total(total as f64)
                                .with_message(message),
                        )
                        .await;
                }
            });
            std::sync::Arc::new(move |step: u64, total: u64, msg: &str| {
                let _ = tx.send((step, total, msg.to_string()));
            }) as crate::engine::OriginProgress
        });

        // A newly added (or adopted) domain may already carry a MANIFEST that
        // declares a `Provisioning` section, so `provisioning_declared` can
        // flip on this call; notify the same way `configure` does for
        // `github.enabled`.
        let before = self.engine.provisioning_declared();
        let result: Result<Value, EngineError> = if let Some(repo) = p.repo.as_deref() {
            self.engine
                .origin_add_with_progress(
                    repo,
                    p.domain.as_deref(),
                    p.path.as_deref(),
                    p.branch.as_deref(),
                    p.folder.as_deref(),
                    progress,
                )
                .await
        } else if p.is_virtual {
            if p.folder.is_some() {
                Err(EngineError::Invalid(
                    "add_domain: a virtual domain has no folder; omit folder or drop virtual"
                        .to_string(),
                ))
            } else {
                match p.domain.as_deref() {
                    Some(domain) => self.engine.domain_add_virtual(domain).await,
                    None => Err(EngineError::Invalid(
                        "add_domain: a virtual domain requires a domain name".to_string(),
                    )),
                }
            }
        } else {
            self.engine
                .domain_add_local(p.domain.as_deref(), p.folder.as_deref())
                .await
        };
        let after = self.engine.provisioning_declared();
        if before != after
            && let Err(e) = ctx.peer.notify_tool_list_changed().await
        {
            tracing::warn!("failed to send tools/list_changed after add_domain: {e}");
        }

        result.map_err(to_error).and_then(ok)
    }

    #[tool(
        name = "share_changes",
        title = "Share changes",
        description = "Share this domain's new knowledge and experience with the team as a proposal they review on GitHub; returns the review URL to hand to the user. Refuses while conflicts are unsettled so the team always reviews a clean proposal.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
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
        title = "Update domain",
        description = "Learn the team's latest knowledge: pulls what was merged upstream into the domain (or every shared domain), merging cleanly where possible and flagging real conflicts for resolve_conflict.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn update_domain(
        &self,
        Parameters(p): Parameters<UpdateDomainParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        // A pull can rewrite a domain's MANIFEST, so `provisioning_declared`
        // can flip here too; notify the same way `add_domain` and `configure`
        // do.
        let before = self.engine.provisioning_declared();
        let result = self.engine.origin_update(p.domain.as_deref()).await;
        let after = self.engine.provisioning_declared();
        if before != after
            && let Err(e) = peer.notify_tool_list_changed().await
        {
            tracing::warn!("failed to send tools/list_changed after update_domain: {e}");
        }
        result.map_err(to_error).and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "origin_status",
        title = "Origin status",
        description = "Review each shared domain's standing: whether the team has new knowledge to learn, what is waiting to be shared, open and declined proposals and any conflicts to settle.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn origin_status(
        &self,
        Parameters(p): Parameters<OriginStatusParams>,
    ) -> Result<CallToolResult, ErrorData> {
        self.engine
            .origin_status(p.domain.as_deref())
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "resolve_conflict",
        title = "Resolve conflict",
        description = "Settle a flagged conflict by keeping your version (mine), taking the team's version (theirs) or providing merged content. The engram then counts as ordinary local knowledge you can share.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
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

    #[tool(
        name = "provision",
        title = "Provision harness artifacts",
        description = "Provision the skills, commands, agents and MCP servers a domain ships into the user's coding harnesses. A domain declares artifact folders in its MANIFEST; each domain needs a one-time allow or deny decision from the user before anything installs. status shows decisions and pending domains, allow or deny records a decision and applies it, apply reconciles updates and removals. Installed artifacts update when the domain's files change and disappear when the domain is denied or removed.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn provision(
        &self,
        Parameters(p): Parameters<ProvisionParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let action = match p.action.as_str() {
            "status" => ProvisionAction::Status,
            "apply" => ProvisionAction::Apply,
            "allow" | "deny" => {
                let Some(domain) = p.domain.clone() else {
                    return Err(ErrorData::invalid_params(
                        format!("provision {} requires domain", p.action),
                        None,
                    ));
                };
                if p.action == "allow" {
                    ProvisionAction::Allow { domain }
                } else {
                    ProvisionAction::Deny { domain }
                }
            }
            other => {
                return Err(ErrorData::invalid_params(
                    format!("provision action must be status, allow, deny or apply, got '{other}'"),
                    None,
                ));
            }
        };
        self.engine
            .provision(&action)
            .await
            .map_err(to_error)
            .and_then(|v| self.ok_list(v))
    }

    #[tool(
        name = "skills",
        title = "Skills",
        description = "List the agent skills this server ships and read any skill's full SKILL.md playbook: how to route, capture, model schemas and collaborate well with Crystalline. Call with no arguments for the index of names and descriptions; pass name to read one skill before its kind of task.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            open_world_hint = false
        )
    )]
    async fn skills(
        &self,
        Parameters(p): Parameters<SkillsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(name) = p.name.as_deref() else {
            let index: Vec<Value> = SKILL_ASSETS
                .iter()
                .map(|s| json!({ "name": s.name, "description": s.description() }))
                .collect();
            return self.ok_list(json!({ "skills": index }));
        };
        match crystalline_core::skill(name) {
            // The playbook is markdown a model reads directly, so it is the
            // single text block verbatim rather than a JSON-wrapped string.
            Some(asset) => Ok(CallToolResult::success(vec![ContentBlock::text(
                asset.content,
            )])),
            None => Err(ErrorData::invalid_params(
                format!(
                    "skills: no skill named '{name}'; this server ships {}",
                    skill_names()
                ),
                None,
            )),
        }
    }
}

/// The two prompts a client can insert verbatim: the live routing block for
/// this server and the static snippet that teaches any client to onboard
/// itself. Declared with the rmcp prompt macros so each one's name,
/// description and (absent) arguments live at its handler; `list_prompts` and
/// `get_prompt` are hand-written in the `ServerHandler` impl instead of
/// generated, so the `skills.serve` gate can empty the list. See the module
/// docs.
#[prompt_router]
impl McpServer {
    /// The routing block the initialize `instructions` also carry, re-rendered
    /// per call: the cache refresh first is what makes a virtual domain's
    /// bullets current, exactly as the daemon does before `get_info`.
    #[prompt(
        name = "onboarding",
        title = "Knowledge routing",
        description = "The live knowledge routing block for this server: one routing line per domain plus the behavior rules. Insert at session start."
    )]
    async fn onboarding_prompt(&self) -> Vec<PromptMessage> {
        self.engine.refresh_routing_cache().await;
        vec![PromptMessage::new_text(
            Role::User,
            self.engine.routing_text(),
        )]
    }

    /// The static bootstrap snippet, identical to what `crystalline prompt
    /// connector` prints: a client whose custom instructions carry it onboards
    /// itself through `list_domains` even when nothing else reaches it.
    #[prompt(
        name = "connector",
        title = "Connector instructions",
        description = "A short static snippet to paste into a client's custom instructions so every session onboards itself through list_domains."
    )]
    async fn connector_prompt(&self) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            Role::User,
            crystalline_core::CONNECTOR_SNIPPET,
        )]
    }
}

impl McpServer {
    /// Wrap a list-shaped engine value as a successful tool result: TOON
    /// under the default `service.response_format`, byte-identical to [`ok`]
    /// under `json`. The format is read per response, so a runtime configure
    /// switch applies from the next tool call on.
    fn ok_list(&self, value: Value) -> Result<CallToolResult, ErrorData> {
        match self.engine.response_format() {
            ResponseFormat::Json => ok(value),
            ResponseFormat::Toon => Ok(CallToolResult::success(vec![ContentBlock::text(
                crate::toon::render(&value),
            )])),
        }
    }

    /// Applies `configure`'s `set` map then `unset` list, one key at a time
    /// through the engine's existing per-key [`ConfigureAction`], stopping at
    /// the first failure. On success every applied key has already taken
    /// effect (and been persisted); on failure the error names which key
    /// failed and which keys before it were already applied, so the caller
    /// never has to guess the resulting state.
    async fn apply_settings(&self, p: &ConfigureParams) -> Result<(), ErrorData> {
        let mut applied: Vec<String> = Vec::new();
        for (key, value) in &p.set {
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
        for key in &p.unset {
            match self
                .engine
                .configure(&ConfigureAction::Unset { key: key.clone() })
                .await
            {
                Ok(_) => applied.push(key.clone()),
                Err(e) => return Err(applied_failure(&applied, key, e)),
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
    /// reads a current cache and never blocks on the store. `server_info` is
    /// also set explicitly: `ServerInfo::default()` leaves
    /// `Implementation::from_build_env()`, which would report the rmcp crate's
    /// own name and version to harness logs rather than crystalline's.
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.server_info = Implementation::new("crystalline", crystalline_core::VERSION);
        let mut instructions = self.engine.routing_text();
        if self.engine.response_format() == ResponseFormat::Toon {
            instructions.push_str(TOON_INSTRUCTIONS_NOTE);
        }
        info.instructions = Some(instructions);
        // Capabilities are initialize facts a session cannot renegotiate, so
        // resources and prompts stay advertised whatever `skills.serve` says;
        // the gate empties the two lists instead of retracting the capability
        // mid-session. Resource subscribe is deliberately not enabled: the
        // shipped skills are static for a binary's lifetime, so there is
        // nothing to subscribe to.
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_resources()
            .enable_resources_list_changed()
            .enable_prompts()
            .enable_prompts_list_changed()
            .build();
        info
    }

    /// Complete the handshake, and decide there and then whether this
    /// connection is one Crystalline has already onboarded by other means.
    ///
    /// This is where the receipt-aware `auto` behaviour lives, for one blunt
    /// reason: `get_info` takes `&self` and no request, so a server cannot see
    /// who is connecting from inside it, while rmcp's `ServerHandler::
    /// initialize` receives the client's own `InitializeRequestParams`. It is
    /// therefore the earliest and the only point at which the instructions
    /// this connection receives can depend on who asked, and it is a plain
    /// trait override rather than a transport-level rewrite: the daemon relay,
    /// the embedded stdio stack and the HTTP transport all serve through this
    /// same handler, so all three behave identically without the raw
    /// JSON-RPC interception in `crate::client` growing a second job.
    ///
    /// The two things rmcp's default implementation does are done here too and
    /// must stay: publishing the peer info (which is what `client_actor` and
    /// every `generated.by` write read afterwards) and echoing a client's
    /// protocol version when it is one rmcp knows.
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        let requested = request.protocol_version.clone();
        let client_name = request.client_info.name.clone();
        context.peer.set_peer_info(request);

        // Only a stdio client is same-machine, so only there does this
        // machine's receipt say anything about what the client already has.
        let matched = self.transport == Transport::Stdio
            && match &self.install_receipt {
                Some(path) => receipt_matches_client(
                    &client_name,
                    &crystalline_core::harnesses_with_hooks(path),
                ),
                None => false,
            };
        self.receipt_matched.store(matched, Ordering::Relaxed);

        let mut info = self.get_info();
        if minimal_instructions(self.engine.skills_serve(), matched) {
            // The client's own session hook delivers the full routing block at
            // session start, so repeating it here would spend the tokens twice.
            // The TOON note is appended all the same: no hook carries it, it
            // describes this connection's wire format rather than the
            // knowledge, and a client that cannot read a tool result is worse
            // off than one that read the routing block twice.
            let mut instructions = crystalline_core::render_minimal_instructions();
            if self.engine.response_format() == ResponseFormat::Toon {
                instructions.push_str(TOON_INSTRUCTIONS_NOTE);
            }
            info.instructions = Some(instructions);
        }
        if !ProtocolVersion::KNOWN_VERSIONS.contains(&requested) {
            tracing::warn!(
                "client requested unsupported protocol version {requested}; serving {}",
                info.protocol_version
            );
        } else {
            info.protocol_version = requested;
        }
        Ok(info)
    }

    /// List the exposed tools. In read-only mode the write-gated tools (the
    /// four content-mutating engram tools plus `add_domain`) are filtered out so
    /// they are absent from `tools/list`, while their routes stay registered for
    /// the call-by-name guard (see `WRITE_TOOLS`). The five collaboration tools
    /// are filtered the same way against the engine's live `github.enabled` and
    /// `read_only` state (see `hidden_collab_tool`), consulted fresh on every
    /// call rather than cached, since `configure` can flip `github.enabled`
    /// mid-session. Both this method and `get_tool` run every surviving
    /// tool's schema through `crate::tool_schema::sanitize_tool` before
    /// returning it, so advertised schemas stay in the conservative
    /// client-compatible shape.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let read_only = self.engine.read_only();
        let github_enabled = self.engine.github_enabled();
        let provisioning_declared = self.engine.provisioning_declared();
        let skills_hidden =
            hidden_skills_surface(self.engine.skills_serve(), self.receipt_matched());
        let mut tools = Self::tool_router().list_all();
        tools.retain(|t| {
            if is_write_tool(&t.name) && read_only {
                return false;
            }
            if is_collab_tool(&t.name) && hidden_collab_tool(&t.name, github_enabled, read_only) {
                return false;
            }
            if t.name == "provision" && hidden_provision_tool(read_only, provisioning_declared) {
                return false;
            }
            if t.name == "skills" && skills_hidden {
                return false;
            }
            true
        });
        for tool in &mut tools {
            crate::tool_schema::sanitize_tool(tool);
        }
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
            let github_enabled = self.engine.github_enabled();
            if hidden_collab_tool(name, github_enabled, read_only) {
                return None;
            }
        }
        if name == "provision"
            && hidden_provision_tool(read_only, self.engine.provisioning_declared())
        {
            return None;
        }
        if name == "skills"
            && hidden_skills_surface(self.engine.skills_serve(), self.receipt_matched())
        {
            return None;
        }
        let mut tool = Self::tool_router().get(name).cloned()?;
        crate::tool_schema::sanitize_tool(&mut tool);
        Some(tool)
    }

    /// List the shipped agent skills as `skill://<name>/SKILL.md` resources,
    /// so a remote client that never runs the CLI can read the same playbooks
    /// an installed harness gets. Empty while `skills.serve` is off, read
    /// fresh here the way the tool gates are.
    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        if hidden_skills_surface(self.engine.skills_serve(), self.receipt_matched()) {
            return Ok(ListResourcesResult::with_all_items(Vec::new()));
        }
        let resources = SKILL_ASSETS
            .iter()
            .map(|s| {
                Resource::new(skill_uri(s.name), s.name)
                    .with_description(s.description())
                    .with_mime_type(SKILL_MIME_TYPE)
            })
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    /// Read one shipped skill by its resource uri. Like every hidden tool,
    /// this answers even while `skills.serve` is off: the gate hides the
    /// surface from a listing rather than disabling it, and a skill is static
    /// public copy this binary already carries, so a client holding a uri from
    /// an earlier listing gets the bytes rather than a puzzle.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        match skill_for_uri(&request.uri) {
            Some(asset) => Ok(ReadResourceResult::new(vec![
                ResourceContents::text(asset.content, &request.uri).with_mime_type(SKILL_MIME_TYPE),
            ])),
            None => Err(ErrorData::invalid_params(
                format!(
                    "unknown resource '{}'; this server serves {}",
                    request.uri,
                    skill_uris()
                ),
                None,
            )),
        }
    }

    /// List the two onboarding prompts, empty while `skills.serve` is off.
    /// Hand-written rather than `#[prompt_handler]`-generated for exactly that
    /// gate: the macro replaces any `list_prompts` in the impl block it is
    /// applied to, so a generated one could never be emptied.
    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, ErrorData> {
        let prompts = if hidden_skills_surface(self.engine.skills_serve(), self.receipt_matched()) {
            Vec::new()
        } else {
            Self::prompt_router().list_all()
        };
        Ok(ListPromptsResult {
            prompts,
            meta: None,
            next_cursor: None,
        })
    }

    /// Render one prompt through the macro-declared router. Answers while the
    /// gate is off for the same reason `read_resource` does.
    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResult, ErrorData> {
        Self::prompt_router()
            .get_prompt(PromptContext::new(
                self,
                request.name,
                request.arguments,
                context,
            ))
            .await
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
    fn is_collab_tool_recognizes_exactly_the_five() {
        for name in COLLAB_TOOLS {
            assert!(is_collab_tool(name), "{name}");
        }
        // add_domain is write-gated, not collab-gated.
        assert!(!is_collab_tool("add_domain"));
        assert!(is_write_tool("add_domain"));
        assert!(!is_collab_tool("write_engram"));
        assert!(!is_collab_tool("search_engrams"));
    }

    /// The whole per-connection decision, one assertion per row of the value
    /// set: `true` and `false` ignore the connection entirely, `auto` follows
    /// it.
    #[test]
    fn hidden_skills_surface_covers_every_setting_and_connection_pair() {
        assert!(!hidden_skills_surface(SkillsServe::Always, false));
        assert!(
            !hidden_skills_surface(SkillsServe::Always, true),
            "true serves an installed harness too, on purpose"
        );
        assert!(hidden_skills_surface(SkillsServe::Never, false));
        assert!(hidden_skills_surface(SkillsServe::Never, true));
        assert!(
            !hidden_skills_surface(SkillsServe::Auto, false),
            "the default serves everyone the receipt does not know"
        );
        assert!(
            hidden_skills_surface(SkillsServe::Auto, true),
            "the default hides the surface from a harness that has it on disk"
        );
    }

    /// Only `auto` plus a match shrinks the instructions: `false` gates skill
    /// serving, never onboarding.
    #[test]
    fn minimal_instructions_are_auto_and_matched_only() {
        assert!(minimal_instructions(SkillsServe::Auto, true));
        assert!(!minimal_instructions(SkillsServe::Auto, false));
        assert!(!minimal_instructions(SkillsServe::Always, true));
        assert!(
            !minimal_instructions(SkillsServe::Never, true),
            "turning the skill surface off must not cost a client its routing block"
        );
        assert!(!minimal_instructions(SkillsServe::Never, false));
    }

    /// The client-name table: the three verified harness names match when the
    /// receipt has them with hooks, every other name never matches.
    #[test]
    fn receipt_matching_is_by_verified_client_name_and_hooked_harness() {
        use crystalline_core::HarnessKind;
        let all = [
            HarnessKind::ClaudeCode,
            HarnessKind::Codex,
            HarnessKind::Copilot,
        ];
        assert!(receipt_matches_client("claude-code", &all));
        assert!(receipt_matches_client("codex-mcp-client", &all));
        assert!(receipt_matches_client("github-copilot-developer", &all));
        // Case-insensitive on the name.
        assert!(receipt_matches_client("Claude-Code", &all));

        // A harness the receipt does not list with hooks never matches.
        assert!(!receipt_matches_client(
            "claude-code",
            &[HarnessKind::Codex]
        ));
        assert!(!receipt_matches_client("claude-code", &[]));

        // An unknown client name never matches, whatever is installed.
        for name in ["", "cursor", "claude", "codex", "copilot", "crystalline"] {
            assert!(
                !receipt_matches_client(name, &all),
                "'{name}' is not a name any onboarded harness sends"
            );
        }
    }

    #[test]
    fn skill_uris_round_trip_to_their_assets() {
        for asset in SKILL_ASSETS {
            let uri = skill_uri(asset.name);
            assert_eq!(uri, format!("skill://{}/SKILL.md", asset.name));
            assert_eq!(skill_for_uri(&uri).map(|a| a.name), Some(asset.name));
        }
        assert!(skill_for_uri("skill://crystalline-routing").is_none());
        assert!(skill_for_uri("skill://nonesuch/SKILL.md").is_none());
        assert!(skill_for_uri("https://example.com/SKILL.md").is_none());
    }

    #[test]
    fn hidden_collab_tool_matches_the_locked_gating_matrix() {
        // disabled + read-write: only configure of the five is visible.
        assert!(!hidden_collab_tool("configure", false, false));
        for name in [
            "share_changes",
            "update_domain",
            "origin_status",
            "resolve_conflict",
        ] {
            assert!(hidden_collab_tool(name, false, false), "{name}");
        }

        // disabled + read-only: none of the five are visible.
        for name in COLLAB_TOOLS {
            assert!(hidden_collab_tool(name, false, true), "{name}");
        }

        // enabled + read-write: all five are visible.
        for name in COLLAB_TOOLS {
            assert!(!hidden_collab_tool(name, true, false), "{name}");
        }

        // enabled + read-only: only update_domain and origin_status are visible.
        for name in ["update_domain", "origin_status"] {
            assert!(!hidden_collab_tool(name, true, true), "{name}");
        }
        for name in ["configure", "share_changes", "resolve_conflict"] {
            assert!(hidden_collab_tool(name, true, true), "{name}");
        }
    }
}
