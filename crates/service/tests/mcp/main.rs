//! One test binary for the MCP suites: instructions, cache hints, skills,
//! subscriptions, the degraded stub, the 2026-07-28 era, streamable HTTP and
//! attachments over MCP.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::support`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../support/mod.rs"]
mod support;

mod http_stream;
mod mcp_attachments;
mod mcp_cache_hints;
mod mcp_instructions;
mod mcp_modern_era;
mod mcp_skills;
mod mcp_subscriptions;
mod stub;
