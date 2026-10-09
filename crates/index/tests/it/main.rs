//! One test binary for index's integration suites.
//!
//! Each module below was a test binary of its own, unchanged, so a test is
//! named `<module>::<test>` and an edit to the index relinks one binary here
//! instead of eighteen. `embed_model` and `nli_model` keep their own
//! `#![cfg(feature = "local-embeddings")]`, which gates the module now.

mod contradictions;
mod embed;
mod embed_model;
mod lead_vectors;
mod nli_model;
mod perf;
mod perf_meta;
mod plans;
mod recovery;
mod reindex;
mod reparse;
mod retired;
mod salience;
mod semantic_split;
mod store;
mod sync_phases;
mod sync_slabs;
mod sync_targeted;
mod turso_only;
