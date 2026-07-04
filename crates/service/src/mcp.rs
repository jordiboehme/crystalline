//! The rmcp tool router: exactly the 12 tools of the v1 MCP surface.
//!
//! Each tool is a thin wrapper over [`crate::engine::Engine`], which does the
//! real work and is shared with the CLI data commands. Tool descriptions are
//! agent-facing product copy framed around onboarding, teaching, learning and
//! experience. The recommended `type` and `status` value sets are stated in the
//! `write_engram` and `edit_engram` descriptions as guidance; they are never
//! enforced. Every mutating tool requires an explicit domain.
//!
//! In read-only mode (the engine's `read_only` flag) the four content-mutating
//! tools are filtered out of `list_tools` and `get_tool`, so the surface is the
//! eight read tools; the routes stay registered so a client that calls a hidden
//! tool by name reaches the engine's read-only guard and gets a clean error.

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, tool, tool_handler, tool_router};
use serde_json::Value;

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

use crate::engine::{Engine, EngineError};
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
}

#[tool_handler]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some(if self.engine.read_only() {
            // Read-only surface: no capture language. Knowledge is curated
            // externally; the agent searches and reads.
            "Crystalline gives you a durable memory across sessions. Domains hold curated knowledge; engrams are the units you read to recall what is known. This deployment is read-only: its knowledge is curated externally, so search and read to learn and do not attempt to write.".to_string()
        } else {
            "Crystalline gives you a durable memory across sessions. Domains hold curated knowledge; engrams are the units you read, write and refine as you work. Search before you write, capture decisions and learnings as engrams, and always name the domain when writing.".to_string()
        });
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }

    /// List the exposed tools. In read-only mode the four content-mutating
    /// tools are filtered out so they are absent from `tools/list`, while their
    /// routes stay registered for the call-by-name guard (see `WRITE_TOOLS`).
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = Self::tool_router().list_all();
        if self.engine.read_only() {
            tools.retain(|t| !is_write_tool(&t.name));
        }
        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    /// Resolve a tool definition by name, hiding the content-mutating tools in
    /// read-only mode so they never surface through `get_tool` either.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        if self.engine.read_only() && is_write_tool(name) {
            return None;
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
        | EngineError::Remote(_) => ErrorData::invalid_params(e.to_string(), None),
        EngineError::Io { .. } | EngineError::Internal(_) => {
            ErrorData::internal_error(e.to_string(), None)
        }
    }
}
