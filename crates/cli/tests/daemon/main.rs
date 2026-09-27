//! One test binary for the daemon suites: the service lifecycle on unix and on
//! Windows and the MCP stub.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::common`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[cfg(unix)]
mod mcp_stub;
#[cfg(unix)]
mod service;
#[cfg(windows)]
mod service_windows;
