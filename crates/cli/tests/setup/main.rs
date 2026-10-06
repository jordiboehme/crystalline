//! One test binary for the setup suites: install, doctor, provision, version,
//! the routing prompt, lifecycle hooks and configure.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::common`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../common/mod.rs"]
mod common;
#[path = "../common/remote_server.rs"]
mod remote_server;

mod configure;
mod doctor;
mod doctor_names;
#[cfg(unix)]
mod harness_install;
#[cfg(unix)]
mod hook;
#[cfg(unix)]
mod install;
mod prompt;
#[cfg(unix)]
mod provision;
#[cfg(unix)]
mod source_hooks;
mod version;
