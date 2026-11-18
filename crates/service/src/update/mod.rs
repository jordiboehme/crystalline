//! `crystalline update`'s network side: the lookup of the newest stable
//! release, the download and verification of its assets, the update lock,
//! the programs `update now` runs, the tar.gz swap, the daemon handover of
//! `update finish` and the daemon's daily check. The
//! pure parts (install method, verification, the cache, the age gate and
//! the reading of GitHub's answers) live in `crystalline_core::update`.

pub mod check;
pub mod client;
pub mod codesign;
pub mod finish;
pub mod now;
pub mod spawn;
pub mod swap;

pub use check::{UpdateState, check_now, check_suppressed, detect_here, read_cache, run_check};
pub use client::{UpdateError, download, latest_release};
pub use codesign::codesign_ok;
pub use finish::{Handover, hand_over};
pub use now::{
    ReleaseSource, UpdateLock, Verified, download_dir, extract_zip_member, fetch_verified,
    fetch_verified_from, remove_older_tags, take_lock,
};
pub use spawn::{Spawner, SystemSpawner};
pub use swap::{
    extract_member, install_tarball, install_verified, root_owned_install_dir, swap, under_sudo,
    version_matches, writable_install_dir,
};
