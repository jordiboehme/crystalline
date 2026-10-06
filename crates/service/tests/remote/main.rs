//! One test binary for domains from shared servers: the server's ctl route,
//! remote tools and answers, revocation, the client's connection and
//! refresh, sign-in, and the local daemon's mount table, routing and fan-out.
//! Every test that needs a server starts a real one on a loopback port
//! through `daemon::http_router`.

#[path = "../support/mod.rs"]
mod support;

mod answers;
mod client;
mod connect;
mod ctl;
mod fixture;
mod live;
pub mod local;
mod revoke;
mod routing;
mod tool;
