//! The shared engine of Crystalline. Data operations run through one
//! [`engine::Engine`], reached over the socket when a daemon owns the index
//! or in-process for a brief standalone open; the MCP tools, the control
//! commands, the JSON API and the CLI data commands all funnel through it.

pub mod collab;
#[doc(hidden)]
pub mod domain_view;
pub mod engine;
pub mod harness_cli;
#[doc(hidden)]
pub mod index_files;
pub mod maintenance;
pub mod nudge;
#[doc(hidden)]
pub mod origin;
pub mod overlay;
#[doc(hidden)]
pub mod overlay_files;
pub mod overlay_journal;
pub mod params;
#[doc(hidden)]
pub mod poller;
#[doc(hidden)]
pub mod review;
#[doc(hidden)]
pub mod serving;
pub mod settings;
#[doc(hidden)]
pub mod share_staging;
pub mod similar;
pub mod subscribers;
pub mod temp_store;
#[doc(hidden)]
pub mod toon;
pub mod web_url;

// The identity crate's modules under the names the moved files already use,
// so every `crate::auth_store::`, `crate::join::` and `crate::scope::` path
// inside this crate resolves exactly as it did in crystalline-service.
pub(crate) use crystalline_identity::{auth_store, join, scope};
