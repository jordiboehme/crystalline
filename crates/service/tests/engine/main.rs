//! One test binary for the engine suites: writes, attachments, similar-engram
//! probes, the graph, index files, sync and watch, activity, the embedding
//! tick, orphaned rows, moves and the model upgrade.
//!
//! Each module below was a test binary of its own. The files are unchanged
//! apart from reaching the shared helpers as `crate::support`, so a test is
//! named `<module>::<test>` and an edit upstream relinks one binary here
//! instead of one per file.

#[path = "../support/mod.rs"]
mod support;

mod activity;
mod attachments;
mod embed_tick;
mod engine_writes;
mod file_stamps;
mod graph;
mod index_files;
mod model_upgrade;
mod move_permalink;
mod orphaned_rows;
mod similar;
#[cfg(unix)]
mod sync_denied;
mod sync_forward_refs;
mod watch_targeted;
