# Everything a crystalline development machine needs, installable in one
# shot with `brew bundle`. Each entry says why it is here; if a tool stops
# earning its line, remove both.

# The Rust toolchain manager. Homebrew links only rustup itself; the cargo,
# rustc, clippy and rustfmt proxies live in /opt/homebrew/opt/rustup/bin,
# which is not on the default PATH (see CLAUDE.md). rustup enforces the
# rust-toolchain.toml pin, so no pinned channel needs installing by hand.
brew "rustup"

# The fast test runner every gate uses: `cargo nextest run -p <crate>`.
# The canonical fallback stays `cargo test --workspace`.
brew "cargo-nextest"

# Compiler cache: makes branch switches and clippy's separate artifact
# universe mostly cache hits instead of full rebuilds. Wired up through
# .cargo/config.toml's rustc-wrapper.
brew "sccache"

# Dependency hygiene: cargo-audit answers the RUSTSEC question the currency
# audits ask; cargo-deny gates licenses and duplicate versions.
brew "cargo-audit"
brew "cargo-deny"

# The Fluid web UI builds with pnpm on node ("cd fluid && pnpm build").
# CI bootstraps pnpm through corepack instead; locally the formulae are
# simpler and match the same lockfile.
brew "node"
brew "pnpm"

# GitHub CLI: PRs, CI watching, release publishing and editing - the whole
# release routine runs through gh.
brew "gh"

# The optional PostgreSQL backend, validated locally before every
# postgres-touching change (three local runs beat one CI guess - the parity
# suite needs a real server with pgvector).
brew "postgresql@18", link: false
brew "pgvector"
