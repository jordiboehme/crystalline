# Claude Desktop

The `.mcpb` extension is the whole install; no terminal is needed.

1. Download `crystalline-v<version>.mcpb` from the [latest release](https://github.com/jordiboehme/crystalline/releases/latest) - one universal bundle covering Apple Silicon Macs and Windows (per-arch bundles remain for Intel Macs and native windows-arm64).
2. In Claude Desktop, open Settings > Extensions > Advanced settings > Install Extension... and pick the file.

It starts with no domains: the agent creates one with the `add_domain` tool whenever it needs somewhere to capture knowledge - a folder of markdown files under your `Documents/Crystalline` folder, a database-backed domain or a GitHub team domain. Onboarding is automatic on every connection (see [Session onboarding](../learning-loop.md#session-onboarding)). The extension gets you the browser half too: the daemon it spawns serves the web UI at `http://localhost:7411` by default, where the first visit creates your admin account - it is there while Desktop is open and goes away five seconds after Desktop quits. The optional companion skill adds capture and collaboration best practices (see [Skills](../learning-loop.md#skills)); the [Claude Desktop extension scenario](../deployment.md#claude-desktop-extension) shows how it works underneath.

## The companion skill

Download `crystalline-claude-desktop-skill-v<version>.zip` from the latest release, then open Settings > Capabilities > Skills (enable the Skills capability there if it is off) and upload the zip as-is (it contains the `crystalline-intelligence` folder; do not unpack it). If you uploaded an earlier release's skill, delete the old `crystalline-memory` entry there once the new one is up - Desktop keeps uploaded skills side by side, and the two teach the same lessons twice. Routing itself needs no skill - the server's instructions deliver it automatically; the skill adds capture and collaboration best practices.
