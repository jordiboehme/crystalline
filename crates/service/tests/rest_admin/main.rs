//! One test binary for the JSON API administration suites: admin routes, the
//! write matrix and draft links.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::support`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../support/mod.rs"]
mod support;

mod rest_admin_api;
mod rest_draft_links;
mod rest_write_api;
