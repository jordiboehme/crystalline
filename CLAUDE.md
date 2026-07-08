# Crystalline

Local-first knowledge management for humans and AI agents. Every Domain has exactly one source of truth: by default Markdown files (Engrams) on disk, or, for a virtual domain, the database itself. The search index is always a disposable derived layer; an MCP server and CLI sit on top.

## Purpose

Crystalline gives an AI agent the capability to learn and evolve instead of starting from zero in every session. An agent is onboarded via a generated routing prompt, taught information through curated domains and stores its learnings and experiences as engrams while it works - becoming a useful and productive peer over time. All user-facing language (README, skills, MCP tool descriptions, routing prompt) is framed around onboarding, teaching, learning and experience rather than notes or documents.

## Vocabulary

- **Domain** - a registered folder of knowledge with a mandatory MANIFEST.md at its root, used for routing
- **Engram** - one markdown file holding a unit of knowledge, with YAML frontmatter (OKF compatible)
- Address scheme: `crystalline://<domain>/<permalink>`

## Workspace

Cargo workspace, Rust edition 2024, pinned toolchain in rust-toolchain.toml.

- `crates/core` (crystalline-core) - format layer: parser, emitter, schema, verify, prompt. Must never depend on async runtimes, databases or ML crates
- `crates/index` (crystalline-index) - Store trait, embedded database backend, sync engine, search, embeddings
- `crates/service` (crystalline-service) - single-instance daemon, MCP server, control protocol
- `crates/cli` (crystalline) - the single user-facing binary

Dependency direction: core <- index <- service <- cli.

## Commands

- Build: `cargo build --release`
- Test: `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check`
- Style check: `bash scripts/style-lint.sh`

## Toolchain

Rust is installed via Homebrew (`brew install rust`), which puts `cargo`, `rustc`, `clippy` and `rustfmt` directly on PATH at `/opt/homebrew/bin`. The commands above run as written, no wrapper needed.

There is no rustup, so the `rust-toolchain.toml` pin (channel 1.96.1) is not enforced: Homebrew supplies whatever version its `rust` formula carries, currently matching the pin at 1.96.1. If a `brew upgrade` moves `rust` past the pinned channel, either accept the drift or install rustup to honor `rust-toolchain.toml`.

## Local folders (gitignored)

- `plans/` - implementation plans. Read the newest plan before starting work; store any new plan here
- `research/` - background research notes

## Hard rules

- Never reference other knowledge-management tools by name anywhere in this repo, neither in code nor in docs, comments or commit messages. The local plans in `plans/` explain the specifics
- Commit messages: plain, human style. No AI attribution of any kind - no co-author trailers, no generated-with lines
- AI harnesses may be named in user-facing docs (README, skills) only where Crystalline integration is documented
- No emdashes or en dashes in any file - use a plain '-'
- No Oxford comma in markdown files or in application strings
- Temporal semantics: absent `valid_from` = has always been valid, absent `valid_to` = valid forever. Never write sentinel dates like 9999-12-31
- `status` and `type` frontmatter fields are required and non-empty but free form - recommended value sets are guidance, never enforced globally
- Commit after each completed milestone or task
- Use the latest stable versions of dependencies and standards; verify on crates.io rather than assuming
- docs/deployment.md holds the deployment documentation: every scenario (text plus one mermaid chart per scenario), the container guide, the environment variable reference and read-only serving; the README keeps a Deployment section with a one-line-per-scenario table linking into it. Any change that adds or alters a deployment mode (new serve flag, new image variant, new compose example, new transport) must update docs/deployment.md and the README table in the same change

## Known upstream workarounds

- **"Wait for upstream" (gemm fp16)**: the `gemm-common` crate (a transitive dependency of the embedding runtime via candle) emits aarch64 fp16 NEON asm without per-function `#[target_feature(enable = "fp16")]` annotations, which fails to assemble against the default arm64 Linux baseline (upstream issue: sarah-quinones/gemm#31). Workaround in place: `-C target-feature=+fp16` scoped to the arm64 Linux matrix legs in `.github/workflows/ci.yml` and `.github/workflows/release.yml`, which raises the arm64 Linux binary baseline to ARMv8.2+ (Raspberry Pi 5 yes, Pi 4 and older ARMv8.0 boards unsupported in principle). When the upstream fix ships in a released gemm version: update the dependency, remove the `rustflags` entry from BOTH matrix legs, confirm the ubuntu-24.04-arm CI leg builds and tests green without it and drop any ARMv8.2 notes from user-facing docs. The crate's runtime feature detection already gates the fp16 kernels correctly, so ARMv8.0 hardware works at full fidelity once the flag is gone
- **"Wait for upstream" (keyring 4.1.3 default store)**: the `keyring` crate's `v1` feature never installs a default credential store, because its one-shot guard in `v1.rs` checks `SET_CREDENTIAL_STORE.compare_exchange(false, true, ..) == Ok(true)`, but `compare_exchange` returns `Ok` of the *previous* value on success, so that check can never be true and the branch that would call `set_credential_store()` is dead code. Every `keyring::Entry::new` call therefore returns `NoDefaultStore` on every platform. Workaround in place: `crates/remote/src/token.rs` installs the platform default store itself, once per process behind a `std::sync::Once` guard, by calling `keyring_core::set_default_store` directly with the platform-native store (Apple Keychain on macOS, Windows Credential Manager on Windows, the Secret Service over `zbus` on other Unix platforms), the same thing keyring's own `cli` feature does. When a fixed `keyring` release ships: bump the dependency, delete the explicit installer function and its `Once` guard from `token.rs`, drop the direct `keyring-core` and platform store crate dependencies these added to `crates/remote/Cargo.toml` and the workspace manifest and confirm `TokenStore::resolve` reaches the keyring arm on macOS instead of always falling back to the file store
