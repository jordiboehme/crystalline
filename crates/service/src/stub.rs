//! The degraded status server: what `crystalline mcp` serves over stdio when
//! the embedded stack cannot start at all (the index lock is held by another
//! instance, the config will not load, the store will not open).
//!
//! A terminal startup failure used to end the process with a `-32000`
//! `initialize` reply, which Claude Desktop renders as "Server disconnected"
//! with no hint about what went wrong or how to fix it. Instead this module
//! serves a minimal, always-startable MCP server: `initialize` succeeds with
//! per-case `instructions`, and a single `status` tool reports the failure,
//! this binary's version, the daemon that owns the index and the fix, so the
//! model can relay it to the user rather than the whole session going dark.
//!
//! The most common cause is an upgrade skew: the Claude Desktop extension
//! bundles its own `crystalline` binary, and a daemon installed another way
//! (a newer brew, say) owns the index at a version the bundled binary is too
//! old to displace. That case gets extension-specific copy pointing at the
//! releases page; a plain second instance gets conflict copy; anything else
//! gets the raw startup error and a pointer at `daemon.log`.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ErrorData, Implementation, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use serde_json::Value;

/// The install-channel marker env var. The mcpb manifest sets it to "mcpb".
pub const CHANNEL_ENV: &str = "CRYSTALLINE_CHANNEL";
/// The Claude Desktop extension channel value.
pub const MCPB_CHANNEL: &str = "mcpb";

/// The description of the single `status` tool, agent-facing product copy: it
/// tells the model to relay the fix to the user rather than keeping it.
const STATUS_TOOL_DESCRIPTION: &str = "Report why Crystalline is degraded this session: the startup failure, this binary's version, the daemon that owns the knowledge index and how to fix it. Relay the fix to the user.";

/// Everything the degraded server knows about why it is degraded, gathered once
/// at startup. The optional fields are present only when a live daemon record
/// explains the failure; absent (not null) otherwise, which drives both the
/// case selection and the tool payload shape.
#[derive(Debug, Clone)]
pub struct StubStatus {
    /// The startup error chain, `format!("{e:#}")` of the failure that forced
    /// the degraded server.
    pub reason: String,
    /// This binary's version, `crystalline_core::VERSION`.
    pub binary_version: String,
    /// The owning daemon's version, kept only when its pid is alive.
    pub daemon_version: Option<String>,
    /// The owning daemon's pid, kept only when it is alive.
    pub daemon_pid: Option<u32>,
    /// The install channel from `CRYSTALLINE_CHANNEL`, when set.
    pub channel: Option<String>,
}

/// Which explanatory copy the degraded server renders, chosen once from the
/// gathered status. Private: the exact copy per case is the public contract,
/// not the discriminant.
enum StubCase {
    /// A strictly newer daemon owns the index and this is the Desktop
    /// extension: point the user at the releases page and an over-install.
    OutdatedMcpb,
    /// A strictly newer daemon owns the index and this is a plain install:
    /// tell the user to update this installation.
    OutdatedBinary,
    /// A live instance owns the index but is not the newer-daemon skew (equal,
    /// older or unparseable version): a plain conflict naming its pid.
    Conflict,
    /// No live record explains the failure: surface the raw startup error and
    /// point at daemon.log.
    Other,
}

impl StubStatus {
    /// Gather the degraded status from the environment: the owning daemon
    /// record (kept only when its pid is still alive, so a dead pid never
    /// masquerades as a conflict) and the install channel. `reason` is the
    /// startup error chain that forced the degraded server.
    pub fn gather(reason: String) -> StubStatus {
        let live = crate::instance::read_lock_info()
            .filter(|info| crate::instance::process_alive(info.pid));
        StubStatus {
            reason,
            binary_version: crystalline_core::VERSION.to_string(),
            daemon_version: live.as_ref().map(|info| info.version.clone()),
            daemon_pid: live.as_ref().map(|info| info.pid),
            channel: std::env::var(CHANNEL_ENV).ok(),
        }
    }

    /// Which case's copy to render. A live daemon record strictly newer than
    /// this binary is the upgrade skew (extension vs plain install by channel);
    /// any other live record is a plain conflict; no live record is generic.
    fn case(&self) -> StubCase {
        match &self.daemon_version {
            Some(daemon) if crate::instance::strictly_newer(daemon, &self.binary_version) => {
                if self.channel.as_deref() == Some(MCPB_CHANNEL) {
                    StubCase::OutdatedMcpb
                } else {
                    StubCase::OutdatedBinary
                }
            }
            Some(_) => StubCase::Conflict,
            None => StubCase::Other,
        }
    }

    /// The `instructions` string handed to the connecting agent, per case. The
    /// model reads this at initialize and relays the fix to the user. The
    /// per-case optional fields are exactly the ones the case selection
    /// guarantees present, so a missing one degrades to an empty substitution
    /// rather than surfacing a placeholder.
    pub fn instructions(&self) -> String {
        let bin = &self.binary_version;
        let daemon = self.daemon_version.as_deref().unwrap_or_default();
        let pid = self.daemon_pid.unwrap_or_default();
        let reason = &self.reason;
        match self.case() {
            StubCase::OutdatedMcpb => format!(
                "Crystalline is running in degraded mode: this Crystalline extension (v{bin}) is older than the Crystalline daemon (v{daemon}) that owns this machine's knowledge, so no knowledge tools are available this session. Ask the user to download the latest Crystalline extension from https://github.com/jordiboehme/crystalline/releases and install it over the current one, then start a new conversation. Call the status tool for the full details to relay."
            ),
            StubCase::OutdatedBinary => format!(
                "Crystalline is running in degraded mode: this crystalline binary (v{bin}) is older than the Crystalline daemon (v{daemon}) that owns this machine's knowledge, so no knowledge tools are available this session. Ask the user to update this Crystalline installation to v{daemon} or newer, then reconnect. Call the status tool for the full details to relay."
            ),
            StubCase::Conflict => format!(
                "Crystalline is running in degraded mode: another Crystalline instance (pid {pid}) owns this machine's knowledge index and it could not be reached, so no knowledge tools are available this session. Ask the user to check or restart that instance, then reconnect. Call the status tool for the full details to relay."
            ),
            StubCase::Other => format!(
                "Crystalline is running in degraded mode: it failed to start ({reason}), so no knowledge tools are available this session. Ask the user to check daemon.log in the Crystalline state directory. Call the status tool for the full details to relay."
            ),
        }
    }

    /// The one-line fix, per case, carried in the tool payload and appended to
    /// the error a stale tool call gets.
    pub fn fix(&self) -> String {
        let daemon = self.daemon_version.as_deref().unwrap_or_default();
        let pid = self.daemon_pid.unwrap_or_default();
        match self.case() {
            StubCase::OutdatedMcpb =>
                "Download the latest Crystalline extension (.mcpb) from https://github.com/jordiboehme/crystalline/releases and install it over the current extension, then start a new conversation.".to_string(),
            StubCase::OutdatedBinary => format!(
                "Update this Crystalline installation to v{daemon} or newer, then reconnect."
            ),
            StubCase::Conflict => format!(
                "Check or restart the Crystalline instance with pid {pid}, then reconnect."
            ),
            StubCase::Other =>
                "Check daemon.log in the Crystalline state directory for the startup error, then reconnect.".to_string(),
        }
    }

    /// The `status` tool's JSON body: `available` is always false, `reason`,
    /// `binary_version` and `fix` are always present, and the daemon fields and
    /// channel are present only when known (absent, never null, otherwise).
    pub fn tool_payload(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("available".to_string(), Value::Bool(false));
        map.insert("reason".to_string(), Value::String(self.reason.clone()));
        map.insert(
            "binary_version".to_string(),
            Value::String(self.binary_version.clone()),
        );
        if let Some(version) = &self.daemon_version {
            map.insert("daemon_version".to_string(), Value::String(version.clone()));
        }
        if let Some(pid) = self.daemon_pid {
            map.insert("daemon_pid".to_string(), Value::Number(pid.into()));
        }
        if let Some(channel) = &self.channel {
            map.insert("channel".to_string(), Value::String(channel.clone()));
        }
        map.insert("fix".to_string(), Value::String(self.fix()));
        Value::Object(map)
    }
}

/// The degraded MCP server: an [`Arc`] status shared across the rmcp handler's
/// cloned copies. Every fallible startup step already ran before this exists,
/// so serving it cannot fail; `initialize`, ping and protocol negotiation come
/// free from rmcp's [`ServerHandler`] defaults.
#[derive(Clone)]
pub struct DegradedServer {
    status: Arc<StubStatus>,
}

impl DegradedServer {
    /// Build a degraded server around the gathered status.
    pub fn new(status: StubStatus) -> DegradedServer {
        DegradedServer {
            status: Arc::new(status),
        }
    }

    /// The single `status` tool: an empty-object input schema (it takes no
    /// arguments) and read-only, closed-world annotations, so a client can
    /// batch it and skip a confirmation prompt.
    fn status_tool() -> Tool {
        let input_schema: JsonObject = match serde_json::json!({ "type": "object", "properties": {} })
        {
            Value::Object(map) => map,
            _ => unreachable!("an object literal is an object"),
        };
        Tool::new("status", STATUS_TOOL_DESCRIPTION, input_schema)
            .with_title("Crystalline status")
            .annotate(
                ToolAnnotations::with_title("Crystalline status")
                    .read_only(true)
                    .open_world(false),
            )
    }
}

impl ServerHandler for DegradedServer {
    /// The degraded handshake: identify as `crystalline` at this binary's
    /// version and hand the connecting agent the per-case degraded copy as its
    /// `instructions`, advertising only the tools capability.
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.server_info = Implementation::new("crystalline", crystalline_core::VERSION);
        info.instructions = Some(self.status.instructions());
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }

    /// The degraded surface is exactly the one `status` tool.
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: vec![Self::status_tool()],
            meta: None,
            next_cursor: None,
        })
    }

    /// Resolve `status` by name; every other name is unknown here.
    fn get_tool(&self, name: &str) -> Option<Tool> {
        (name == "status").then(Self::status_tool)
    }

    /// `status` returns the compact JSON payload as a single text block, the
    /// same single-text-block convention the healthy server uses. Any other
    /// tool name - a client replaying a stale tool list from a healthy session
    /// will call `search_engrams` and the like - gets a tool-level error (not a
    /// protocol `Err`, which clients render opaquely) carrying the instructions
    /// and the fix, so the model learns why the call failed and what to relay.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if request.name == "status" {
            let text = serde_json::to_string(&self.status.tool_payload())
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
        } else {
            let text = format!("{}\n\n{}", self.status.instructions(), self.status.fix());
            Ok(CallToolResult::error(vec![ContentBlock::text(text)]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a status directly (no lock, no env): unit tests never touch the
    /// process environment, which is global and would race nextest's
    /// concurrent tests; the channel is the struct field instead.
    fn status(
        daemon_version: Option<&str>,
        daemon_pid: Option<u32>,
        channel: Option<&str>,
    ) -> StubStatus {
        StubStatus {
            reason: "cannot run an embedded MCP server: another Crystalline instance owns the index (pid 4242)".to_string(),
            binary_version: "0.8.2".to_string(),
            daemon_version: daemon_version.map(str::to_string),
            daemon_pid,
            channel: channel.map(str::to_string),
        }
    }

    #[test]
    fn newer_daemon_on_the_mcpb_channel_points_at_the_releases_page() {
        let s = status(Some("0.9.0"), Some(4242), Some(MCPB_CHANNEL));
        let instructions = s.instructions();
        assert!(
            instructions.contains("https://github.com/jordiboehme/crystalline/releases"),
            "instructions carry the releases URL:\n{instructions}"
        );
        assert!(
            instructions.contains("install it over the current"),
            "instructions tell the user to over-install:\n{instructions}"
        );
        let fix = s.fix();
        assert!(
            fix.contains("https://github.com/jordiboehme/crystalline/releases"),
            "fix carries the releases URL:\n{fix}"
        );
        assert!(
            fix.contains("install it over the current"),
            "fix tells the user to over-install:\n{fix}"
        );
    }

    #[test]
    fn newer_daemon_without_a_channel_tells_the_user_to_update_this_installation() {
        let s = status(Some("0.9.0"), Some(4242), None);
        let instructions = s.instructions();
        assert!(
            instructions.contains("update this Crystalline installation to v0.9.0 or newer"),
            "instructions name the target version:\n{instructions}"
        );
        assert!(
            s.fix()
                .contains("Update this Crystalline installation to v0.9.0 or newer"),
            "fix names the target version:\n{}",
            s.fix()
        );
    }

    #[test]
    fn an_equal_version_live_record_is_a_plain_conflict() {
        let s = status(Some("0.8.2"), Some(4242), None);
        let instructions = s.instructions();
        assert!(
            instructions.contains("owns this machine's knowledge index"),
            "conflict copy:\n{instructions}"
        );
        assert!(
            instructions.contains("pid 4242"),
            "names the pid:\n{instructions}"
        );
        assert!(
            !instructions.contains("update this Crystalline installation"),
            "no binary-update hint:\n{instructions}"
        );
        assert!(
            !instructions.contains("install it over the current"),
            "no over-install hint:\n{instructions}"
        );
        assert!(
            s.fix().contains("pid 4242"),
            "fix names the pid:\n{}",
            s.fix()
        );
    }

    #[test]
    fn an_unparseable_record_version_is_a_plain_conflict() {
        let s = status(Some("garbage"), Some(4242), Some(MCPB_CHANNEL));
        let instructions = s.instructions();
        assert!(
            instructions.contains("owns this machine's knowledge index"),
            "conflict copy despite the mcpb channel:\n{instructions}"
        );
        assert!(
            instructions.contains("pid 4242"),
            "names the pid:\n{instructions}"
        );
        assert!(
            !instructions.contains("install it over the current"),
            "an unparseable version is never the newer-daemon skew:\n{instructions}"
        );
    }

    #[test]
    fn no_live_record_carries_the_raw_reason() {
        let s = status(None, None, None);
        let instructions = s.instructions();
        assert!(
            instructions.contains(&s.reason),
            "generic copy carries the reason:\n{instructions}"
        );
        assert!(
            instructions.contains("daemon.log"),
            "generic copy points at daemon.log:\n{instructions}"
        );
        assert!(
            s.fix().contains("daemon.log"),
            "generic fix points at daemon.log:\n{}",
            s.fix()
        );
    }

    #[test]
    fn tool_payload_omits_unknown_keys_and_keeps_known_ones() {
        let s = status(Some("0.9.0"), Some(4242), Some("mcpb"));
        let payload = s.tool_payload();
        assert_eq!(payload["available"], serde_json::json!(false));
        assert_eq!(payload["reason"], serde_json::json!(s.reason));
        assert_eq!(payload["binary_version"], serde_json::json!("0.8.2"));
        assert_eq!(payload["daemon_version"], serde_json::json!("0.9.0"));
        assert_eq!(payload["daemon_pid"], serde_json::json!(4242));
        assert_eq!(payload["channel"], serde_json::json!("mcpb"));
        assert_eq!(payload["fix"], serde_json::json!(s.fix()));

        let bare = status(None, None, None).tool_payload();
        let obj = bare.as_object().unwrap();
        assert_eq!(obj["available"], serde_json::json!(false));
        assert!(
            !obj.contains_key("daemon_version"),
            "absent, not null: {bare}"
        );
        assert!(!obj.contains_key("daemon_pid"), "absent, not null: {bare}");
        assert!(!obj.contains_key("channel"), "absent, not null: {bare}");
        assert!(obj.contains_key("fix"), "fix is always present: {bare}");
    }

    #[test]
    fn the_status_tool_declares_an_empty_schema_and_a_read_only_annotation() {
        let tool = DegradedServer::status_tool();
        assert_eq!(tool.name, "status");
        let schema = tool.input_schema.as_ref();
        assert_eq!(schema.get("type"), Some(&serde_json::json!("object")));
        assert!(
            schema
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|p| p.is_empty()),
            "empty properties: {schema:?}"
        );
        let annotations = tool.annotations.as_ref().expect("annotations present");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
    }

    #[test]
    fn no_produced_string_carries_an_em_or_en_dash() {
        let cases = [
            status(Some("0.9.0"), Some(4242), Some(MCPB_CHANNEL)),
            status(Some("0.9.0"), Some(4242), None),
            status(Some("0.8.2"), Some(4242), None),
            status(Some("garbage"), Some(4242), None),
            status(None, None, None),
        ];
        for s in &cases {
            for text in [s.instructions(), s.fix(), s.tool_payload().to_string()] {
                assert!(!text.contains('\u{2014}'), "em dash in:\n{text}");
                assert!(!text.contains('\u{2013}'), "en dash in:\n{text}");
            }
        }
    }
}
