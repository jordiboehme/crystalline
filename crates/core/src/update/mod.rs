//! The pure parts of `crystalline update`: how this binary was installed and
//! which release asset fits it. Nothing here touches the network, spawns a
//! process or needs a runtime; the caller hands in the canonical exe path and
//! an environment lookup.

pub mod install;

pub use install::{
    CHANNEL_ENV, Detection, InstallMethod, MARKER_FILE, Refusal, archive_member, asset_name,
    detect, official_build, release_platform,
};
