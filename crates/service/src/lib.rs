//! `crystalline-service` owns the single running instance of Crystalline for a
//! machine: the advisory lock and socket that guarantee exactly one process
//! holds the derived index, the daemon that watches Domains and runs the
//! embedding queue, the ctl control protocol and the rmcp tool router.
//!
//! The CLI is a thin dispatcher over this crate. Data operations run through one
//! shared [`engine::Engine`], reached either over the socket (when a daemon owns
//! the index) or in-process (a brief standalone open). The 12 MCP tools, the ctl
//! commands and the CLI data commands all funnel through that one engine.

pub mod client;
pub mod control;
pub mod daemon;
pub mod engine;
pub mod instance;
pub mod mcp;
mod origin;
pub mod overlay;
pub mod params;
mod poller;
pub mod settings;

pub use client::{
    configure, ctl_if_running, ctl_required, domain_export, domain_import, origin_add,
    origin_discard, origin_resolve, origin_share, origin_status, origin_update, run_mcp, run_tool,
    scaffold_virtual_manifest, use_daemon, virtual_routing_bullets,
};
pub use daemon::run_serve;
pub use engine::{Engine, EngineError};
pub use mcp::McpServer;
pub use origin::parse_origin_spec;
pub use overlay::{EnvDomain, EnvOverlay, LoadedConfig};
