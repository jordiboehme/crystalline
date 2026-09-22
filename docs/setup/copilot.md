# GitHub Copilot CLI

The same integration for the agentic Copilot CLI, one command.

Copilot too keeps MCP registration user-level even with `--project`. The installer drives the `copilot` binary and falls back to `gh copilot` when only the GitHub CLI form is installed:

```sh
crystalline install copilot
```

Hooks land in a dedicated `~/.copilot/hooks/crystalline.json` and skills in `~/.copilot/skills` (both honor `COPILOT_HOME`); with `--project` they go to `.github/hooks` and `.github/skills` instead, which Copilot loads once you trust the folder. Then start a session and say the prompt from the README's [Get started](../../README.md#get-started).

## After an upgrade

`crystalline install copilot` does not repair a Copilot registration written before the `--harness` flag existed. Replace the entry yourself, `copilot mcp remove crystalline && copilot mcp add crystalline -- crystalline mcp --harness copilot`, or leave it: why the installer keeps its hands off it, and what leaving it costs, is in [Skills over MCP](../learning-loop.md#skills-over-mcp).

## The recall hook

The per-prompt recall hook is installed for Copilot too, but Copilot has no channel yet to show what it found: see [The learning loop](../learning-loop.md#the-learning-loop).
