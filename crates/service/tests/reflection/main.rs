//! One test binary for the Reflection: the switch, reflections, threads, the
//! reflect queue, rights, the MCP tools, the REST contract, co-editing rooms
//! and the backup branch. Modules are added by the tasks that need them.

#[path = "../support/mod.rs"]
mod support;

mod assets;
mod assets_mcp;
mod backup;
mod disclosure;
mod disk_changes;
mod docs;
mod fixture;
mod guards;
mod hosts;
mod kept;
mod mode_change;
mod nudges;
mod own_writes;
mod queue;
mod reach;
mod read_limit;
mod rest;
mod rights;
mod search;
mod sources;
mod switch;
mod threads;
