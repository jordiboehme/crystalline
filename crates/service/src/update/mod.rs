//! `crystalline update`'s network side: the lookup of the newest stable
//! release, the download of its assets and the daemon's daily check. The pure parts (install method,
//! verification, the cache, the age gate and the reading of GitHub's
//! answers) live in `crystalline_core::update`.

pub mod check;
pub mod client;

pub use check::{UpdateState, check_suppressed, read_cache, run_check};
pub use client::{UpdateError, download, latest_release};
