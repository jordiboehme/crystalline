//! One test binary for core's integration suites.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::common`, so a test is
//! named `<module>::<test>` and an edit to core relinks one binary here
//! instead of fifteen.

mod common;

mod address;
mod asset_ref_corpus;
mod base_routes;
mod config;
mod conventional_corpus;
mod editing;
mod linked;
mod manifest;
mod names;
mod orchestrate;
mod provision;
mod reconcile;
mod reflection_asset_corpus;
mod roundtrip;
mod schema;
mod structure;
mod time_boundary;
mod time_differential;
mod translate;
mod verify_smoke;
