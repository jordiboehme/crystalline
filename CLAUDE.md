# Crystalline

Local-first knowledge management for humans and AI agents. Every Domain has exactly one source of truth: by default Markdown files (Engrams) on disk, or, for a virtual domain, the database itself. The search index is always a disposable derived layer; an MCP server and CLI sit on top.

## Purpose

Crystalline gives an AI agent the capability to learn and evolve instead of starting from zero in every session. An agent is onboarded via a generated routing prompt, taught information through curated domains and stores its learnings and experiences as engrams while it works - becoming a useful and productive peer over time. All user-facing language (README, skills, MCP tool descriptions, routing prompt) is framed around onboarding, teaching, learning and experience rather than notes or documents.

Positioning follows the psychology concept of fluid vs crystallized intelligence: a language model is fluid intelligence (reasons in the moment, keeps nothing), Crystalline is the crystalline intelligence an agent accumulates and keeps, and the web UI (Fluid) is where the two meet. Describe the product as "crystalline intelligence"; the agent-facing phrase avoids the Crystalline/crystalline doubling and reads "Crystalline is your crystallized intelligence across sessions". Never describe it as "crystalline memory" or a memory tool.

## Vocabulary

- **Domain** - a registered folder of knowledge with a mandatory MANIFEST.md at its root, used for routing
- **Engram** - one markdown file holding a unit of knowledge, with YAML frontmatter (OKF compatible)
- Address scheme: `crystalline://<domain>/<permalink>`

## Workspace

Cargo workspace, Rust edition 2024, pinned toolchain in rust-toolchain.toml.

- `crates/core` (crystalline-core) - format layer: parser, emitter, schema, verify, prompt. Must never depend on async runtimes, databases or ML crates
- `crates/index` (crystalline-index) - Store trait, embedded database backend, sync engine, search, embeddings
- `crates/remote` (crystalline-remote) - GitHub-backed team collaboration plumbing: the forge-neutral Provider trait, the plain-text merge engine and the on-disk origin state. Git is never invoked; everything goes through the GitHub APIs
- `crates/identity` (crystalline-identity) - accounts, roles, domain access, joins and scope
- `crates/engine` (crystalline-engine) - the shared engine, review-mode overlay, origins, co-editing sessions
- `crates/rest` (crystalline-rest) - the JSON API, sign-in and the co-editing socket
- `crates/service` (crystalline-service) - single-instance daemon, MCP server, control protocol; the facade the CLI and the integration tests reach everything through
- `crates/cli` (crystalline) - the single user-facing binary

Dependency direction: core <- index <- identity <- engine <- rest <- service <- cli, with remote beside index (it depends on core and is used by engine, service and cli).

## Commands

- Build: `cargo build --release`
- Test (fast path): `cargo nextest run --workspace` (install: `brew install cargo-nextest`), plus doctests via `cargo test --workspace --doc`
- Test (canonical fallback): `cargo test --workspace`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check`
- Style check: `bash scripts/style-lint.sh`
- Fluid (from `fluid/`): `npm run build`, `npm test -- --run`, `npm run lint`, `pnpm run format:check`, `pnpm typecheck`; CI runs all five, so a change that skips the format check or the typecheck goes red on main. A change that adds, moves or renames a control also runs the browser smoke, `bash fluid/e2e/run-smoke.sh` (needs `pnpm build`, `cargo build -p crystalline` and the chromium install; on a mac export `CRYSTALLINE_TEST_NO_KEYCHAIN=1` first), and greps `fluid/e2e/*.spec.ts` for the accessible names it changed

## Toolchain

Rust is managed by rustup, installed via Homebrew (`brew install rustup`). Homebrew links only `rustup` itself into `/opt/homebrew/bin`; the proxies for `cargo`, `rustc`, `clippy` and `rustfmt` live in `/opt/homebrew/opt/rustup/bin`, which is not on the default PATH. If `cargo` is not found, prepend that directory (`export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`) and the commands above run as written.

rustup enforces the `rust-toolchain.toml` pin: inside this repo every proxy resolves to channel 1.98.0 (including clippy and rustfmt) regardless of the default toolchain, and a missing pinned toolchain is downloaded on first use. That file is the authority; if this line and the pin ever disagree, the pin wins and this line is stale.

## Local folders (gitignored)

- `plans/` - implementation plans. Read the newest plan before starting work; store any new plan here
- `research/` - background research notes. Store any research produced while working here

## Hard rules

- Never reference other knowledge-management tools by name anywhere in this repo, neither in code nor in docs, comments or commit messages. The local plans in `plans/` explain the specifics
- Commit messages: plain, human style. No AI attribution of any kind - no co-author trailers, no generated-with lines
- AI harnesses may be named in user-facing docs (README, skills) only where Crystalline integration is documented
- No emdashes or en dashes in any file - use a plain '-'
- The Oxford comma is allowed (the ban was lifted on 2026-08-04); existing text was not rewritten, so both list styles appear in the tree
- Temporal semantics: absent `valid_from` = has always been valid, absent `valid_to` = valid forever. Never write sentinel dates like 9999-12-31
- `status` and `type` frontmatter fields are required and non-empty but free form - recommended value sets are guidance, never enforced globally
- Every MANIFEST configuration key gets a registry entry (`policy_registry` in crates/core/src/manifest.rs) and a row in the domain policies card in the same change; a key with no registry entry fails the registry guard test, and the card draws its rows from the registry, so a key never has to be drawn by hand
- Commit after each completed milestone or task
- Use the latest stable versions of dependencies and standards; verify on crates.io rather than assuming
- docs/deployment.md holds the deployment documentation: every scenario (text plus one mermaid chart per scenario), the container guide, the environment variable reference and read-only serving; the README keeps a Deployment section with a one-line-per-scenario table linking into it. Any change that adds or alters a deployment mode (new serve flag, new image variant, new compose example, new transport) must update docs/deployment.md and the README table in the same change
- MCP tool descriptions double as the client-side tool-search corpus, not only per-session context: keep them keyword-rich and prescriptive about when to call the tool. Do not rely on a description alone to make a new tool get called, though: measured on the evolve benchmark (2026-08-04), a deliberately keyword-rich description with no skill guidance beside it produced zero calls in sixteen opportunities across two independent runs, and rewriting it to front-load the triggers and add the missing vocabulary changed nothing. Skill text is what drives discovery of a novel verb, so budget for a sentence in the skill whenever a new tool needs to be found rather than merely understood
- Store every implementation plan in plans/ with a dated filename before starting work; store research notes in research/
- Delegate implementation to subagents with the model matched to task complexity: opus for design-heavy or intricate work, sonnet for routine or mechanical work. The orchestrator reviews, gates and commits
- A new type lives in the lowest crate that can name every type in its signature; identity never names the engine or a route, the engine never names a route, rest never reaches the daemon, the MCP router or the control socket

## Known upstream workarounds

- **"Wait for upstream" (gemm fp16)**: the `gemm-common` crate (a transitive dependency of the embedding runtime via candle) emits aarch64 fp16 NEON asm without per-function `#[target_feature(enable = "fp16")]` annotations, which fails to assemble against the default arm64 Linux baseline (upstream issue: sarah-quinones/gemm#31). Workaround in place: `-C target-feature=+fp16` scoped to the arm64 Linux matrix legs in `.github/workflows/ci.yml` and `.github/workflows/release.yml`, which raises the arm64 Linux binary baseline to ARMv8.2+ (Raspberry Pi 5 yes, Pi 4 and older ARMv8.0 boards unsupported in principle). When the upstream fix ships in a released gemm version: update the dependency, remove the `rustflags` entry from BOTH matrix legs, confirm the ubuntu-24.04-arm CI leg builds and tests green without it and drop any ARMv8.2 notes from user-facing docs. The crate's runtime feature detection already gates the fp16 kernels correctly, so ARMv8.0 hardware works at full fidelity once the flag is gone
- **RESOLVED (rmcp bare `server/discover` probe)**: rmcp 3.1.2's stdio init loop dropped a first request that was not `initialize` when its `_meta` lacked either SEP-2575 key - no response, closed pipe, which a client reads as a hang. `crates/service/src/client.rs` worked around it by rewriting a bare probe into the shape rmcp would answer and forwarding it. **Upstream fixed it in rmcp 3.1.4 (rust-sdk #1157, PR #1160)**: the loop now sends a `-32602` naming the missing keys before it gives up, so a bare probe gets an error instead of silence. The whole bridge was deleted on the 3.2.0 bump - `DiscoverProbe`, `discover_probe`, `probe_meta`, `normalize_discover_probe`, `observe_discover_probe`, the two `META_KEY_*` constants, the rewrite in `read_session_opener`, the relay hook with `RelayState`'s `opener_already_classified` field and its hand-written `Default` - and `a_bare_discover_probe_is_answered_by_rmcp_and_a_complete_one_returns_our_discover_result` replaces the pin: a bare probe is answered by rmcp, a complete one reaches our `discover()` and comes back as a `DiscoverResult`. `Prefixed` and `initialize_error_reply` were never part of the bridge and stay. Nothing about a probe is classified, rewritten or logged on our side any more; every client line is forwarded verbatim.

  **rmcp 3.2.0 also closed the handshake route into the era, and that reversed a decision of ours.** `negotiate_protocol_version` (`service/server.rs:479`) now echoes a requested revision only when it is a legacy one and otherwise answers the server's newest legacy revision, and `is_legacy_request` (`tower.rs:359-416`) routes any `InitializeRequest` through the session branch before it reads a version at all. So an `initialize` naming 2026-07-28 is answered 2025-11-25 and gets a session, where 3.1.2 echoed the era back and routed it statelessly. The reasoning is upstream's own and it is the one this repo already applied to an unknown version string: `initialize` is deleted from the 2026-07-28 schema, so a peer that sent one is speaking the legacy lifecycle whatever it names. What is lost is the non-standard opt-in "reach the modern lifecycle through a handshake"; the era is still reached the way the specification provides for, through `server/discover` and inline requests carrying the `_meta`. Five tests were rebaselined onto this in the same change (`mcp_instructions` x2, `http_stream`, `mcp_modern_era`, `stub`).

  **rmcp 3.4.0 (2026-09-17) walked the same checklist with nothing to rebaseline**: `uses_legacy_lifecycle`, `is_legacy_version` and `negotiate_protocol_version` are byte-identical to 3.2.0, `is_legacy_request` only moved to a let-chain, elicitation is unchanged, and the whole MCP battery passed as written; the one forced change was the rename of the deprecated `ServerInfo`/`ClientInfo` aliases to `ServerConfig`/`ClientConfig`, which `-D warnings` rejects. The checklist for the next bump: re-read rmcp's two `uses_legacy_lifecycle` clauses against `holder_of` in `mcp.rs`, the version negotiation against the paragraph above, and run the MCP battery before touching a test.

  **The 2026-07-28 era adoption, kept because it is still what the server does.** `SERVED_PROTOCOL_VERSIONS` (`mcp.rs`) lists five revisions, `V_2024_11_05` first and `V_2026_07_28` last, and the ordering is load bearing (`newest_served_protocol_version()` reads `.last()`, `newest_legacy_handshake_version()` `rfind`s below the era). Four conformance fixes carry named tests on both transports: **V1** one tool list for every client, with gates that refuse at call time instead of hiding a route (SEP-2567 list invariance); **V2** onboarding delivered through `server/discover`, the sole instructions channel at the era because `InitializeResult` is deleted from the modern schema; **V3** `list_changed` only to a subscriber, through `subscriptions/listen`; **V4** the version surface - `ping` removed for a modern peer and still answered for a legacy one, sessions for any client that opens with `initialize` whatever revision it names (since rmcp 3.2.0; it was "only a legacy-declaring client" under 3.1.2) and none on the stateless modern path, cache hints (`resultType`/`ttlMs`/`cacheScope`) on every list-shaped result. Two costs Jordi signed off: a default install's tool list grew from **17 to 22** tools at the migration and stands at **20** in 0.18.0 (`mcp_tools.rs` pins it): every gated tool is listed and refuses when called instead of vanishing, except the six collaboration tools, which are withheld while `github.enabled` is off; and receipt-aware suppression of the skills surface is not a handshake-time decision (`clientInfo` is optional under the era and SHOULD NOT drive behaviour) but per-harness gating resolved once at process start from the `--harness` argument the registration carries plus this machine's install receipt. The residue is one client-side deduplication (a dual-era harness pulling a routing block its session hook already delivered) that belongs to `crystalline install`'s hooks, not to the server. One configuration fact from the same migration, kept because the code is otherwise its only record: the MCP streamable-HTTP request body was unbounded before the 3.x line and is now capped at 10 MiB, `with_max_request_body_bytes(crate::rest::MAX_BODY_BYTES)` in `daemon.rs`, the same ceiling the REST API enforces.

- **Vendored ModernBERT (granite embedding model)**: candle-transformers 0.11.0's `modernbert.rs` hardcodes the gated-MLP activation as GeGLU and a `model.` tensor prefix, so it loads `ibm-granite/granite-embedding-97m-multilingual-r2` (SwiGLU, bare backbone) and returns silently wrong vectors (measured at cosine 0.85 against the vendor's ONNX export, the spike saw 0.78 to 0.88 across its fixture). Workaround in place: `crates/index/src/embed/modernbert.rs` is that file vendored with the two upstream patches (huggingface/candle#3991 for the activation, #3992 for the prefix) and the `candle` -> `candle_core` rename, plus three mechanical differences the file's own header lists (this workspace's rustfmt, one `#[allow(clippy::upper_case_acronyms)]` on `ClassifierPooling` and the granite config test); `crates/index/tests/embed_model.rs`'s parity test pins it against `tests/fixtures/granite-parity.json`. When a candle release carries both PRs: bump the workspace pin, delete the vendored file and its `mod` line, point `local.rs` at `candle_transformers::models::modernbert::{Config, ModernBert}`, run the parity test by hand and confirm it still passes to 1e-5, and drop this entry.
