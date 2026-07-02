//! `crystalline-service` owns the single running instance of Crystalline
//! for a machine: the advisory lock and socket that guarantee exactly one
//! process holds the database, the daemon that watches Domains and runs
//! the embedding queue, the control protocol and the MCP tool router. It
//! is empty in this milestone; the service lands in a later milestone.
