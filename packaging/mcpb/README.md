# MCP Bundle packaging

This folder holds `generate-manifest.sh`, which writes the `manifest.json` for a Crystalline MCP Bundle (`.mcpb`) given a platform, a version and an output directory. It does not build anything itself; it only describes an already-built `crystalline` binary to Claude Desktop and other MCPB hosts.

The `mcpb` job in `.github/workflows/release.yml` calls this script once per platform, stages the release binary next to the generated manifest, then runs `@anthropic-ai/mcpb validate` and `@anthropic-ai/mcpb pack` to produce the four `.mcpb` files attached to each release.

Each release also attaches two skill zips packaged by the same job: `crystalline-skill-v<version>.zip` holds the consolidated `crystalline-memory` skill Claude Desktop users upload under Settings > Capabilities > Skills, and `crystalline-skills-v<version>.zip` holds the four topical skills for harnesses that install skill folders directly. The `.mcpb` bundle itself never carries a skill: MCPB has no skill payload and the manifest has no instructions field, so the model-facing onboarding lives in the server's runtime initialize instructions. `crystalline-memory` is a curated summary of the four topical skills; a change to any of those should be checked against `skills/crystalline-memory/SKILL.md` for drift.
