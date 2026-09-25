//! Who a caller is and what they may touch: the account store with its
//! roles, sessions, tokens and grants, the per-domain access rules, and the
//! joins into somebody else's draft. No HTTP surface and no engine live
//! here; the engine and the JSON API both read these types.

pub mod auth_store;
pub mod join;
pub mod scope;

pub use scope::OWNER_IDENTITY_NAME;
