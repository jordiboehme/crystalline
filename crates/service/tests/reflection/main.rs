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
mod display_names;
mod docs;
mod fixture;
mod guards;
mod hosts;
mod kept;
mod mark_stale;
mod mode_change;
mod nudges;
mod own_writes;
mod page_lifecycle;
mod page_rights;
mod page_view;
mod queue;
mod reach;
mod read_limit;
mod reconcile_state;
mod rest;
mod rest_pages;
mod rights;
mod search;
mod sources;
mod stale_queue;
mod switch;
mod threads;
mod unattended;
