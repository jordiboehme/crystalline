# MCP Bundle packaging

This folder holds `generate-manifest.sh`, which writes the `manifest.json` for a Crystalline MCP Bundle (`.mcpb`) given a platform, a version and an output directory. It does not build anything itself; it only describes an already-built `crystalline` binary to Claude Desktop and other MCPB hosts.

The `mcpb` job in `.github/workflows/release.yml` calls this script once per platform, stages the release binary next to the generated manifest, then runs `@anthropic-ai/mcpb validate` and `@anthropic-ai/mcpb pack` to produce the four `.mcpb` files attached to each release.
