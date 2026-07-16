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
- Test (fast path): `cargo nextest run --workspace` (install: `brew install cargo-nextest`), plus doctests via `cargo test --workspace --doc`
- Test (canonical fallback): `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check`
- Style check: `bash scripts/style-lint.sh`

## Toolchain

Rust is managed by rustup, installed via Homebrew (`brew install rustup`). Homebrew links only `rustup` itself into `/opt/homebrew/bin`; the proxies for `cargo`, `rustc`, `clippy` and `rustfmt` live in `/opt/homebrew/opt/rustup/bin`, which is not on the default PATH. If `cargo` is not found, prepend that directory (`export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`) and the commands above run as written.

rustup enforces the `rust-toolchain.toml` pin: inside this repo every proxy resolves to channel 1.96.1 (including clippy and rustfmt) regardless of the default toolchain, and a missing pinned toolchain is downloaded on first use.

## Local folders (gitignored)

- `plans/` - implementation plans. Read the newest plan before starting work; store any new plan here
- `research/` - background research notes. Store any research produced while working here

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
- Store every implementation plan in plans/ with a dated filename before starting work; store research notes in research/
- Delegate implementation to subagents with the model matched to task complexity: opus for design-heavy or intricate work, sonnet for routine or mechanical work. The orchestrator reviews, gates and commits

## Known upstream workarounds

- **"Wait for upstream" (gemm fp16)**: the `gemm-common` crate (a transitive dependency of the embedding runtime via candle) emits aarch64 fp16 NEON asm without per-function `#[target_feature(enable = "fp16")]` annotations, which fails to assemble against the default arm64 Linux baseline (upstream issue: sarah-quinones/gemm#31). Workaround in place: `-C target-feature=+fp16` scoped to the arm64 Linux matrix legs in `.github/workflows/ci.yml` and `.github/workflows/release.yml`, which raises the arm64 Linux binary baseline to ARMv8.2+ (Raspberry Pi 5 yes, Pi 4 and older ARMv8.0 boards unsupported in principle). When the upstream fix ships in a released gemm version: update the dependency, remove the `rustflags` entry from BOTH matrix legs, confirm the ubuntu-24.04-arm CI leg builds and tests green without it and drop any ARMv8.2 notes from user-facing docs. The crate's runtime feature detection already gates the fp16 kernels correctly, so ARMv8.0 hardware works at full fidelity once the flag is gone
- **"Wait for upstream" (rmcp 2.2.0 pre-initialize probe)**: `rmcp`'s server init loop exits without writing a JSON-RPC response when the first request over stdio is anything other than `initialize` or `ping`. The TypeScript MCP SDK's `versionNegotiation.mode = "auto"` path sends a `server/discover` request before `initialize`, which Claude Desktop chat mode ships as of July 2026. rmcp parses it as a `CustomRequest`, hits the `ExpectedInitializeRequest` guard in its `service/server.rs`, returns `Err` without replying and the process exits; the SDK's probe window then reads the closed connection as a `network-error` rather than a legacy fallback signal, never retries with `initialize` and the chat session hangs (code mode uses Claude Code's own client which sends plain `initialize`, so it is unaffected). A `-32601 Method not found` reply would instead be read as a legacy signal and the SDK would proceed with `initialize`. Tracking: TypeScript SDK PR #2466 (2026-07-08) covers the `mode: "auto"` against a legacy server case; `rmcp` 2.2.0 (2026-07-08) does not fix it (verified against the 2.1.0...2.2.0 diff: `service/server.rs` is untouched), and rmcp PR #943 (open, SEP-2575 stateless MCP) adds `server/discover` model types but defers the server wiring - once a release answers `server/discover` natively, delete the interception outright so a probing client gets real discovery and modern version negotiation instead of the legacy fallback. Workaround in place: `crates/service/src/client.rs` intercepts a `server/discover` request on both stdio paths (the daemon relay `relay_loop` and the shared pre-init drain in `run_mcp`) and answers `-32601` itself, never forwarding the probe to the daemon or to rmcp; `run_mcp` drains the probe at the top for both paths, concurrently with daemon acquisition, then re-primes stdin through a `Prefixed` reader so the real `initialize` line feeds whichever path serves - the daemon relay or `rmcp::serve_server`. When a released `rmcp` answers an unknown pre-initialize request with a JSON-RPC error instead of dropping the connection (or the client stops probing legacy servers): bump the dependency, delete `preinit_probe_reply`, `Prefixed`, `drain_preinit_probes`, the two interception hooks in `relay_loop` and `run_mcp` and their five tests from `client.rs` and confirm Claude Desktop chat mode still connects without the interception
