# Codex CLI

The same integration as Claude Code, one command.

Codex keeps MCP registration user-level even with `--project`; the installer says so when it applies:

```sh
crystalline install codex
```

Then start a session and say the prompt from the README's [Get started](../../README.md#get-started).

## After an upgrade

`crystalline install codex` does not repair a Codex registration written before the `--harness` flag existed. Replace the entry yourself, `codex mcp remove crystalline && codex mcp add crystalline -- crystalline mcp --harness codex`, or leave it: why the installer keeps its hands off it, and what leaving it costs, is in [Skills over MCP](../learning-loop.md#skills-over-mcp).
