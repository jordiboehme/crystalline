//! One test binary for the knowledge suites: data verbs, tags, import, verify,
//! the index-access census and environment domains.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::common`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../common/mod.rs"]
mod common;

mod data;
mod env_domains;
mod import;
mod index_access;
mod tags;
mod verify;
