//! `crystalline update`'s network side: the lookup of the newest stable
//! release, the download and verification of its assets, the update lock,
//! the programs `update now` runs and the daemon's daily check. The
//! pure parts (install method, verification, the cache, the age gate and
//! the reading of GitHub's answers) live in `crystalline_core::update`.

pub mod check;
pub mod client;
pub mod now;
pub mod spawn;

pub use check::{UpdateState, check_now, check_suppressed, detect_here, read_cache, run_check};
pub use client::{UpdateError, download, latest_release};
pub use now::{UpdateLock, download_dir, fetch_verified, remove_older_tags, take_lock};
pub use spawn::{Spawner, SystemSpawner};
