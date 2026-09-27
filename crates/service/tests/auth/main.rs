//! One test binary for the sign-in suites: OAuth, OpenID Connect, OAuth grants
//! and MCP authentication.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::support`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../support/mod.rs"]
mod support;

mod mcp_auth;
mod oauth;
mod oidc;
mod rest_oauth_grants;
