# Kiro

The same integration as Claude Code, one command.

```sh
crystalline install kiro
```

`crystalline install` never edits instruction files such as CLAUDE.md, AGENTS.md, GEMINI.md or QWEN.md. It writes only these files, and only for you (user scope, `--project` is refused):

- `~/.kiro/settings/mcp.json`: the `crystalline` entry under `mcpServers`, started with `--harness kiro`.
- `~/.kiro/hooks/crystalline.json`: a hook file Crystalline creates and owns, with the session-start hook that runs `crystalline prompt system`.
- `~/.kiro/steering/crystalline.md`: a steering file Crystalline creates and owns. It has `inclusion: always` and points the agent at the routing block. A file of that name without the Crystalline marker is yours and is left alone.
- `~/.kiro/skills`: the shipped skills.

Before an existing file is changed, the installer keeps a copy of it under `<state dir>/backups/kiro/`, once per file path. Only you can read these copies.

The install writes the full path of the `crystalline` it finds on your PATH (or of the binary you ran) instead of the bare name, because the Kiro IDE may start with a short PATH. Run the install again after you move or upgrade the binary.

## Uninstall

`crystalline uninstall kiro` takes out the MCP entry and the skills it wrote, and deletes the two files it owns. A steering file without the marker is kept unless you pass `--force`. The one exception to a byte-for-byte give-back: an empty `mcpServers` object you had before the install goes away too.

## Known limits

- Kiro CLI 2.x and custom agents may not run hooks from `~/.kiro/hooks`. They rely on the steering file.
- The Windows paths are not verified.
- Until a live check confirms this harness, the skills also stay available over MCP.

Run `crystalline uninstall kiro` before you downgrade Crystalline below 0.22.1.
