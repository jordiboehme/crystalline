# Claude Code

One command wires Claude Code to Crystalline; this page says what it wired and how to undo it.

```sh
crystalline install claude-code
```

One command wires the whole integration: MCP registration, the `SessionStart` onboarding hook, the `Stop` capture nudge, the `UserPromptSubmit` recall hook (see [The learning loop](../learning-loop.md#the-learning-loop)) and the four topical skills. It is idempotent - rerun it any time and whatever is already correct is left untouched - and each part is skippable with `--skip-mcp`, `--skip-hooks` or `--skip-skills`; `--project` writes into the current repository's config instead of your global one, and `crystalline uninstall claude-code` reverses everything `install` did, leaving any hook, key or locally edited skill that is not Crystalline's own in place.

The README's [Get started](../../README.md#get-started) is exactly this path end to end.

Start a session and the agent onboards itself: its `SessionStart` hook hands it one routing line per domain (see [Session onboarding](../learning-loop.md#session-onboarding)). Then open `http://localhost:7411`: the daemon the session started serves the web UI there, and the first visit creates your admin account in the browser. Semantic search needs the local embedding model. It downloads in the background on first start, `crystalline model download` fetches it ahead of time, and plain text search works before it is there. Claude Desktop needs no binary at all: [Claude Desktop](claude-desktop.md).
