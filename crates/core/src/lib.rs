//! `crystalline-core` holds the parts of Crystalline that must stay static:
//! the Engram markdown format, permalink and address logic, the Manifest
//! model, Picoschema and configuration types. This crate intentionally has
//! no async, database or ML dependencies, so `crystalline verify` and
//! `crystalline prompt` can run without a service, a socket or a network
//! connection.

/// The crate version, re-exported from the value Cargo embeds at build
/// time so callers do not need to depend on `env!` directly.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod engram;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!VERSION.is_empty());
    }
}
