//! `crystalline update`'s network side: the lookup of the newest stable
//! release and the download of its assets. The pure parts (install method,
//! verification, the cache, the age gate and the reading of GitHub's
//! answers) live in `crystalline_core::update`.

pub mod client;

pub use client::{UpdateError, download, latest_release};
