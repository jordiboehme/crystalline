# Gemini CLI and Qwen Code

The same integration as Claude Code, one command per harness.

```sh
crystalline install gemini
crystalline install qwen
```

`crystalline install` never edits instruction files such as CLAUDE.md, AGENTS.md, GEMINI.md or QWEN.md. It writes only the files below, and only for you (user scope, `--project` is refused). With `--skip-hooks` the routing block still reaches the agent, through the MCP server instructions.

Before an existing file is changed, the installer keeps a copy of it under `<state dir>/backups/<harness>/`, once per file path. Only you can read these copies. Both harnesses use the bare `crystalline` command, which they find on your PATH.

## Gemini CLI

- `~/.gemini/settings.json`: the `crystalline` entry under `mcpServers`, started with `--harness gemini`, and a `SessionStart` hook that runs `crystalline prompt system --format hook-specific`. Both live in the one file.
- The skills: in `~/.agents/skills`, shared with Codex and Cursor. One copy serves them all.

## Qwen Code

- `~/.qwen/settings.json`: the same two entries, started with `--harness qwen`.
- `~/.qwen/skills`: the shipped skills.

## Uninstall

`crystalline uninstall gemini` and `crystalline uninstall qwen` take out the entries and the skills they wrote and leave the rest of the settings file as it was. A shared skills folder is emptied only when no installed harness still writes it. The one exception to a byte-for-byte give-back: an empty `mcpServers` object you had before the install goes away too.

## Known limits

- Qwen Code may need skills enabled before it loads them.
- The Windows paths are not verified.
- Until a live check confirms a harness, its skills also stay available over MCP.

Run `crystalline uninstall <harness>` before you downgrade Crystalline below 0.22.1.
