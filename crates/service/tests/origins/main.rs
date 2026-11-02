//! One test binary for the origin suites: GitHub collaboration through the in-
//! memory forge, the origin poller and the GitHub App's credential seam.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::support`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../support/mod.rs"]
mod support;

mod github_access;
mod github_app;
mod github_governed;
mod github_link;
mod github_members;
mod github_webhook;
mod origin;
mod origin_names;
mod poller;
mod reflection_reach;
