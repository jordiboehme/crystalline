//! Real-time co-editing sessions: one yrs document per open engram, served
//! over the axum WebSocket route in [`ws`], saved through the engine's own
//! write path. Sessions are in-memory only - the file stays the source of
//! truth, and a daemon restart drops sessions by design (clients rejoin from
//! the saved file).

pub mod control;
pub mod session;
pub mod text;
