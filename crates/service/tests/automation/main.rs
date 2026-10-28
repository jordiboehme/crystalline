//! One test binary for the loops that run themselves: whether a loop is
//! due, the lease a runner takes, the run it records and the pull request it
//! shares. Modules are added by the tasks that need them.

#[path = "../support/mod.rs"]
mod support;

#[path = "../reflection/fixture.rs"]
mod fixture;

mod ctl;
mod engine;
mod evolve_home;
mod trigger;
