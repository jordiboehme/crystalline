//! One test binary for remote's integration suites.
//!
//! Each module below was a test binary of its own, unchanged apart from
//! reaching the GitHub stand-in as `crate::mock` and the shared `HOME` lock
//! as `crate::common`, so a test is named `<module>::<test>`.

mod common;
mod mock;

mod discard;
mod github_auth;
mod github_client;
mod lifecycle;
