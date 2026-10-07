# MCP Bundle packaging

Each release attaches two Claude Desktop extensions (`.mcpb`), both built by `generate-manifest.sh` and packed by the `mcpb` job in `.github/workflows/release.yml`:

- `crystalline-v<version>.mcpb` is the standard extension for macOS and Windows. It carries no binary. Its server is `server/shim.js`, a small Node launcher that Claude Desktop runs with its own Node. The launcher finds the Crystalline that Homebrew (macOS) or the MSI (Windows) installed, checks that it is 0.24.0 or newer and relays stdio to `crystalline mcp`. Without such a binary it answers by itself with one `status` tool that says what to install. The launcher lives in `shim/`, with its `node --test` suite; CI runs it in the `mcpb` job.
- `crystalline-desktop-only-v<version>-macos.mcpb` is for people who use only Claude Desktop on a Mac, not Claude Code or the `crystalline` command. It carries one universal `crystalline` binary (arm64 and x86_64, joined by the `macos-universal` job) and sets `CRYSTALLINE_CHANNEL=mcpb`. That binary lies inside Claude Desktop's extension folder, so the daemon it starts stays attached to Desktop and exits five seconds after its last client, as since 0.18.2.

Both have the same `name`, so Claude Desktop treats a switch from one to the other as an update. There are no per-arch bundles and no Linux bundle: the launcher is the same on every arch, and Claude Desktop is not available on Linux.

`tests/check-manifests.sh` generates both manifests and checks what each must carry. `generate-manifest.sh standard` stages `server/shim.js` itself; for `desktop-only` the caller stages `server/crystalline`.

Each release also attaches two skill zips packaged by the same job: `crystalline-claude-desktop-skill-v<version>.zip` holds the consolidated `crystalline-intelligence` skill Claude Desktop users upload under Settings > Capabilities > Skills, and `crystalline-agent-skills-v<version>.zip` holds the topical skills for harnesses that install skill folders directly. The `.mcpb` bundles carry no skill payload: MCPB has no skill field. The server serves the same skills over MCP (the `skills` tool, `skill://` resources and the `onboarding` and `connector` prompts). `crystalline-intelligence` is a summary of the topical skills; a change to any of those should be checked against `skills/crystalline-intelligence/SKILL.md` for drift.
