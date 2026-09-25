//! One test binary for the team suites: origins, users, domain members, domain
//! review, connect and the environment token.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::common`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../common/mod.rs"]
mod common;

mod connect;
mod domain_members;
mod domain_review;
mod env_token;
mod origin;
mod users;
