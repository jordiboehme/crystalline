//! One test binary for the daemon suites: the service lifecycle on unix and on
//! Windows and the MCP stub.
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

#[cfg(unix)]
mod automation;

// The package and task seams are read only in debug builds, so a release test
// build of these would ask the real package check and the real Task Scheduler.
#[cfg(debug_assertions)]
mod bridge;
#[cfg(unix)]
mod mcp_stub;
// The same two seams as bridge above.
#[cfg(debug_assertions)]
mod packaged;
#[cfg(unix)]
mod service;
// The task seam is read only in debug builds, so a release test build would
// start the real daemon through the real Task Scheduler.
#[cfg(all(windows, debug_assertions))]
mod service_windows;
