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
mod daemon_task;
mod desktop_bundle;
mod desktop_state;
// The update opens the extension: like install_desktop below, only in debug
// builds, where the open seam is read.
#[cfg(all(debug_assertions, any(target_os = "macos", windows)))]
mod desktop_update;
mod doctor;
mod doctor_names;
mod domain_case;
#[cfg(unix)]
mod harness_install;
#[cfg(unix)]
mod hook;
#[cfg(unix)]
mod install;
// The open seam is read only in debug builds, so a release test build would open the real Claude Desktop.
#[cfg(all(debug_assertions, any(target_os = "macos", windows)))]
mod install_desktop;
mod kept;
mod prompt;
#[cfg(unix)]
mod provision;
#[cfg(unix)]
mod source_hooks;
mod version;
mod wix;
