//! One test binary for the MCP suites: instructions, cache hints, skills,
//! subscriptions, the degraded stub, the 2026-07-28 era, streamable HTTP,
//! attachments over MCP and a draft's conflict over MCP.
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
mod mcp_origin_drafts;
mod mcp_read_only;
mod mcp_rename_question;
mod mcp_skills;
mod mcp_subscriptions;
mod mcp_uploads;
mod stub;
