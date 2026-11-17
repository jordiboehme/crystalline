//! The pure parts of `crystalline update`: how this binary was installed,
//! which release asset fits it, how a download is verified, the check's
//! cache and the release-age gate. Nothing here touches the network, spawns a
//! process or needs a runtime; the caller hands in the canonical exe path and
//! an environment lookup.

pub mod cache;
pub mod install;
pub mod verify;

pub use cache::{
    CheckOutcome, Gate, UPDATE_CACHE_FILE, UPDATE_CACHE_FORMAT, UpdateCache, age_gate,
};
pub use install::{
    CHANNEL_ENV, Detection, InstallMethod, MARKER_FILE, Refusal, archive_member, asset_name,
    detect, official_build, release_platform,
};
pub use verify::{Checks, VerifyError, verify_digest, verify_signature, verify_sums_line};
