//! One test binary for the domain suites: evolve, configure, domain
//! administration and access, visibility, TOON measurement and the virtual and
//! shared-backend domains.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::support`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../support/mod.rs"]
mod support;

mod collaboration;
mod configure;
mod domain_access;
mod domain_admin;
mod evolve;
mod evolve_twins;
mod toon_measurement;
mod virtual_domains;
mod visibility;
