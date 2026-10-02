# Cursor

The same integration as Claude Code, one command.

```sh
crystalline install cursor
```

`crystalline install` never edits instruction files such as CLAUDE.md, AGENTS.md, GEMINI.md or QWEN.md. It writes only these files, and only for you (user scope, `--project` is refused):

- `~/.cursor/mcp.json`: the `crystalline` entry under `mcpServers`, started with `--harness cursor`.
- `~/.cursor/hooks.json`: a `sessionStart` hook that runs `crystalline prompt system --format cursor`, so a new chat gets the routing block.
- The skills: in `~/.agents/skills`, shared with Codex and Gemini CLI. Cursor also reads `~/.claude/skills`, so with Claude Code installed it writes no skills of its own and uses that folder.

Before an existing file is changed, the installer keeps a copy of it under `<state dir>/backups/cursor/`, once per file path. Only you can read these copies.

Cursor is a desktop app and may start with a short PATH. So the install writes the full path of the `crystalline` it finds on your PATH (or of the binary you ran) instead of the bare name. Run the install again after you move or upgrade the binary. Restart Cursor after the install so it loads the hook.

## Uninstall

`crystalline uninstall cursor` takes out the MCP entry, the hook and the skills it wrote, and leaves everything else in those files as it was. The one exception: an empty `mcpServers` object you had before the install goes away too, because it is empty after our entry is removed.

## Known limits

- Cursor also runs your Claude Code hooks. The Claude Code routing hook stays silent inside Cursor, so the routing block arrives once.
- With Claude Code installed together with Codex or Gemini CLI, Cursor shows each skill twice, because it reads both folders.
- Until a live check confirms this harness, the skills also stay available over MCP.
- Cursor starts the MCP server with only `HOME` and `PATH` in its environment. A Crystalline set up through `XDG_CONFIG_HOME`, `XDG_STATE_HOME` or `CRYSTALLINE_*` variables is not seen there, and Cursor talks to an empty instance instead. Use the default locations, or set those variables where Cursor gets its environment from.
- The Windows paths are not verified.

Run `crystalline uninstall cursor` before you downgrade Crystalline below 0.22.1.
