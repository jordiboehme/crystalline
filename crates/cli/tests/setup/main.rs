//! One test binary for the setup suites: install, doctor, provision, version,
//! the routing prompt, lifecycle hooks and configure.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::common`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../common/mod.rs"]
mod common;
#[cfg(unix)]
#[path = "../common/daemon_env.rs"]
mod daemon_env;
#[path = "../common/remote_server.rs"]
mod remote_server;

mod configure;
// The task seam is read only in debug builds, so a release test build would
// ask the real Task Scheduler.
#[cfg(debug_assertions)]
mod daemon_task;
// Plants a Claude Desktop package folder, so doctor checks the daemon task on
// Windows, and one test needs the package seam: both seams are read only in
// debug builds.
#[cfg(debug_assertions)]
mod desktop_state;
mod doctor;
mod doctor_names;
mod domain_case;
#[cfg(unix)]
mod harness_install;
#[cfg(unix)]
mod hook;
#[cfg(unix)]
mod install;
mod kept;
mod prompt;
#[cfg(unix)]
mod provision;
#[cfg(unix)]
mod source_hooks;
mod version;
mod wix;
