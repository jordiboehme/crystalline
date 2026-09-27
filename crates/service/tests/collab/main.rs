//! One test binary for the co-editing suites: agents in rooms, saves,
//! sessions, the wire format and MCP co-editing.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::support`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file. Two members stay their own standalone binaries
//! under the threaded proof (`cargo test --workspace`): `collab_merge` is
//! CPU-bound and does not sleep, starving `mcp_collab`'s two-second
//! `wait_until` budget when they share a process; `collab_ws` runs real
//! WebSocket sockets and background tasks that do the same to that same
//! budget once paired with `mcp_collab`, bisected pairing by pairing.

#[path = "../support/mod.rs"]
mod support;

mod collab_agent;
mod collab_saves;
mod collab_session;
mod collab_wire;
mod mcp_collab;
