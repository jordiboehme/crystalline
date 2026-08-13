//! `crystalline-service` owns the single running instance of Crystalline for a
//! machine: the advisory lock and socket that guarantee exactly one process
//! holds the derived index, the daemon that watches Domains and runs the
//! embedding queue, the ctl control protocol, the rmcp tool router and the
//! JSON API the HTTP endpoint mounts at `/api/v1`.
//!
//! The CLI is a thin dispatcher over this crate. Data operations run through one
//! shared [`engine::Engine`], reached either over the socket (when a daemon owns
//! the index) or in-process (a brief standalone open). The MCP tools, the ctl
//! commands and the CLI data commands all funnel through that one engine.

pub mod client;
pub mod collab;
pub mod control;
pub mod daemon;
pub mod engine;
pub mod harness_cli;
mod index_files;
pub mod instance;
pub mod mcp;
mod origin;
pub mod overlay;
pub mod params;
mod poller;
pub mod rest;
pub mod settings;
pub mod stub;
pub mod temp_store;
mod tool_schema;
mod toon;
#[cfg(feature = "fluid-ui")]
pub mod ui;

/// The name the consolidation sweep is advertised and dispatched under.
///
/// The router literal in [`mcp`], the `dispatch_engine` arm behind the ctl
/// `tool` command and the CLI's `evolve` verb all resolve through this one
/// constant, and a test in `crates/service/tests/mcp_tools.rs` asserts the
/// router advertises exactly it. A rename that misses the `#[tool(name = ...)]`
/// literal therefore fails CI instead of silently leaving the tool unrouted.
pub const EVOLVE_TOOL_NAME: &str = "evolve_engrams";

pub use client::{
    configure, ctl_if_running, ctl_required, domain_export, domain_import, origin_add,
    origin_discard, origin_resolve, origin_share, origin_status, origin_update, run_mcp, run_tool,
    scaffold_virtual_manifest, tags_retag, use_daemon, virtual_routing_bullets,
};
pub use daemon::run_serve;
pub use engine::{Engine, EngineError};
pub use harness_cli::{CliRun, SystemMcpRunner, run_harness_cli};
pub use mcp::McpServer;
pub use origin::{default_domain_folder, parse_origin_spec};
pub use overlay::{EnvDomain, EnvOverlay, LoadedConfig};
pub use stub::{DegradedServer, StubStatus};
