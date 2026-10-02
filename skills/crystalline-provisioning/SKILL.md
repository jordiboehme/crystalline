---
name: crystalline-provisioning
description: Use when a person wants a Crystalline domain to ship its own skills, slash commands, agents or MCP servers into their coding harnesses - setting up the tools repo beside the domain, laying out the folders, bringing in skills they already have, adding or changing the MANIFEST Provisioning section, asking for the allow or deny decision, or updating provisioned tools after a change or a git pull.
---

# Crystalline Provisioning

A domain can ship the tools its knowledge depends on - skills, commands, agents and MCP servers - into every coding harness the person has installed Crystalline into. The person makes every decision; you do the legwork and show each step before you take it.

## Before you start

Call `provision` with `action: "status"` first. Stop and say why when the Crystalline server does not run on the person's own machine (hand them the `crystalline provision` commands to run there instead), when `status` lists no harness (they run `crystalline install <harness>` first) or when the domain is virtual (it cannot ship tools). Call `origin_status`: if the domain has a team origin, choose the location from "Team domains" below.

## Choose where the tools live

For a local domain the tools live in a git repo beside the domain folder, by default `<domain-folder>-tools`, declared from the MANIFEST as `../<domain-folder>-tools/<kind>`. If the person names an existing repo, propose `git clone <url> <path>` with that path; for a new one, propose `git init <path>`. Show the exact path and command and run it only after a yes.

## Lay out the folders

Create one folder per kind, and only the kinds the person will use:

- `skills/<name>/SKILL.md` with frontmatter `name` equal to the folder name and a one-line `description`. Scripts and references inside the skill folder ship with it.
- `commands/<namespace>/<name>.md`, frontmatter optional (`description`, `argument-hint`).
- `agents/<name>.md` with frontmatter `name` and `description`; the body is the agent's instructions. Write agents in markdown and never ship `x.md` and `x.toml` for one agent.
- `mcps/<name>.json`, a JSON object with `name` and a `server` object: stdio as `command`, `args` and `env`, HTTP as `"type": "http"`, `url` and `headers`.

```json
{ "name": "tide-tables", "server": { "command": "tide-mcp", "args": ["--stdio"] } }
```

Skills reach all seven harnesses: Claude Code, Codex, GitHub Copilot CLI, Cursor, Kiro, Gemini CLI and Qwen Code. Agents and MCP servers reach Claude Code, Codex and GitHub Copilot CLI only, and commands reach Claude Code and Codex only, so prefer a skill unless the person uses only those harnesses. An HTTP server that needs a header does not work in Codex. For the other harnesses, tell the person to register an MCP server in that harness themselves. Write real files: symlinks and anything whose name starts with `.` never ship.

## Name things

Use lowercase letters, digits and hyphens. A name never starts with `.` and never contains `/`, `\` or `:`. Never use the `crystalline-` prefix: those names belong to `crystalline install`. If another domain already ships a name, pick another. Do not put `ns/name.md` beside `ns-name.md` in commands.

## Bring in skills you already have

Look for candidate skills in the person's harness folders. One folder can serve several harnesses, so list each folder once:

| Harness | Skills folders |
| --- | --- |
| Claude Code | `~/.claude/skills`, project `.claude/skills` |
| Codex | `~/.agents/skills`, project `.agents/skills` |
| GitHub Copilot CLI | `~/.copilot/skills` (or `$COPILOT_HOME/skills`), project `.github/skills` |
| Cursor | `~/.agents/skills`, `~/.cursor/skills`, `~/.claude/skills`, `~/.codex/skills` |
| Kiro | `~/.kiro/skills` |
| Gemini CLI | `~/.agents/skills`, `~/.gemini/skills` |
| Qwen Code | `~/.qwen/skills` |

Skip every skill named `crystalline-*` and every skill `status` already shows as provisioned. Show the person the list with each skill's description and let them choose. Read every chosen skill before you copy it, scripts included, and say what each script runs. Copy, never move, and keep the folder name. Before `allow`, tell the person that the original in the harness folder becomes managed by Crystalline, that a later `deny` removes it, and that an original they changed after the copy is left alone, so the repo copy does not install there.

To bring in an MCP server the person already registered, rewrite it as an `mcps` file and leave out every secret value (see "Never").

## Declare the folders in the MANIFEST

This is a separate step the person sees. Say which bullets you will write, one per kind, each `kind: path` relative to the MANIFEST. Then call `read_engram` with `"identifier": "manifest"` and the domain, and `edit_engram` with the `expected_checksum` from that read.

For a new section use `"operation": "append"` with `## Provisioning` and the bullets as content:

```json
{ "tool": "edit_engram", "arguments": { "domain": "harbor", "identifier": "manifest", "operation": "append",
  "content": "## Provisioning\n\n- skills: ../harbor-tools/skills\n- mcps: ../harbor-tools/mcps\n",
  "expected_checksum": "<from read_engram>" } }
```

For an existing section use `"operation": "replace_section"` with `"section": "## Provisioning"` and the bullets only, without the heading. Read `manifest_findings` on the receipt and fix every finding before going on. A wrong `../` path raises no finding; the counts in "Check the result" catch it.

## Ask for the decision

Call `provision` `status`, then summarize in one sentence what the domain would ship, with the counts per kind, and ask whether to allow or deny. Call `provision` only after the person answers:

```json
{ "tool": "provision", "arguments": { "action": "allow", "domain": "harbor" } }
```

A request to set up provisioning is not consent to allow it.

## Check the result

Call `provision` `status` and compare each declared kind's count with what you laid out; a zero for a declared kind means its path is wrong. Relay every notice from the apply to the person: skipped artifacts, dropped fields, an MCP name already registered outside Crystalline (they remove it first) and any command they must run by hand. With a shell, also run `crystalline doctor`.

## Change or update later

Change artifacts in the tools repo, never in the harness folders: a hand edit there is replaced on the next apply and survives only as a `.bak` file. After an edit, or after a `git pull` you proposed and the person confirmed, call `provision` with `action: "apply"` (or run `crystalline provision`) right away, because MCP servers and harnesses without session hooks change only then. Before a `deny`, say that it removes everything the domain shipped, adopted originals included. Commit or push in the tools repo only when the person asks.

## Team domains

Put the tools inside the domain folder (`skills: skills`, `mcps: mcps`): they travel with `share_changes` like any file and reach teammates with `update_domain`.

Use `../` only when the person confirms the domain is a subfolder of its repository and the target stays inside that repository; ask, because no tool reports it. Never point `../` above the repository root: once merged, it breaks `update_domain` for every teammate.

A `../` folder is read from the team repository, not from your disk: it ships only after it is merged upstream, and `share_changes` does not carry it. Propose committing those files with git and a pull request.

A separate tools repo works only as a domain of its own, which each person subscribes or registers and declares in its own MANIFEST. Every teammate still decides allow or deny on their own machine. `crystalline doctor` shows whether the artifact mirror for a `../` folder is present.

## Never

- Never write a secret value (token, password, API key) into an `mcps` file or any artifact: the `server` object lands in clear in the harness config and in the repo. Tell the person which variable the server reads and where to set it, and leave the value out.
- Never ship a skill, command, agent or MCP server you have not read: it runs with the person's permissions.
- Never decide `allow` or `deny`, and never take a setup request as the decision.
- Never run `git clone`, `git init`, `git pull`, commit or push without showing the path and command and getting a yes.
- Never move a skill out of a harness folder; copy it.
- Never edit provisioned files inside a harness folder; edit the source in the tools repo.
- Never put provisioning text into the person's instruction files; the MANIFEST section is the only declaration.

## Quick reference

- Any provisioning request -> `provision` `status` first, then `origin_status`.
- Tools location -> propose the path and the `git clone` or `git init` command, run it on a yes.
- New section -> `read_engram` `manifest`, `edit_engram` `append` with `expected_checksum`; existing -> `replace_section`, body only.
- Receipt carries `manifest_findings` -> fix them before going on.
- Allow or deny -> one-sentence summary with counts, call `provision` only after the person answers.
- After an edit or `git pull` -> `provision` `apply` at once.
- Team domain -> folders inside the domain; `../` only within the repository root.
