//! One test binary for the JSON API suites: the core routes, visibility, files,
//! evolve, setup, MCP tokens, the OpenAPI snapshot, the CORS and body-cap
//! guards and the served web UI.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::support`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file. `login_throttle` stays its own standalone binary:
//! it measures wall-clock timing against a half-second margin, and sharing a
//! process with the other 202 members' load pushes past that margin under
//! the threaded proof (`cargo test --workspace`), even though no single
//! sibling member reproduces it paired alone.

#[path = "../support/mod.rs"]
mod support;

mod nginx_body_cap;
mod no_cors;
mod openapi_snapshot;
mod rest_api;
mod rest_evolve;
mod rest_files;
mod rest_mcp_tokens;
mod rest_setup_api;
mod rest_visibility;
#[cfg(feature = "fluid-ui")]
mod ui_serving;
