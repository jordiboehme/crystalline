//! Engram is the unit of knowledge in Crystalline: one markdown file with
//! YAML frontmatter, stored inside a Domain. This module will hold the
//! frontmatter and body model, the round-trip parser (unknown keys and key
//! order preserved), observation and relation extraction, wikilink
//! resolution, and the deterministic emitter that writes an Engram back to
//! disk. It is empty in this milestone; the format is designed in a later
//! milestone.
