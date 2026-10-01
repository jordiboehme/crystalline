//! Real-time co-editing sessions: one yrs document per open engram, served
//! over the axum WebSocket route in `ws`, saved through the engine's own
//! write path. Sessions are in-memory only - the file stays the source of
//! truth, and a daemon restart drops sessions by design (clients rejoin from
//! the saved file).
//!
//! The shared text is LF only, the one line ending Crystalline stores: the
//! client binding equates CodeMirror offsets with Y.Text UTF-16 offsets 1:1
//! and CodeMirror counts a line break as one unit, so a CRLF pair inside the
//! shared text would corrupt the mapping. A CRLF or mixed file opens as its
//! LF text and is saved as LF, the whole file, on the room's first save.

pub mod control;
pub mod merge;
pub mod session;

pub use session::{IDLE_CHECK_MS, SAVE_DEBOUNCE_MS, SAVE_MAX_LAG_MS};
