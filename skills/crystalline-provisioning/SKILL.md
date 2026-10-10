---
name: crystalline-provisioning
description: Use when a person wants a Crystalline domain to ship its own skills, slash commands, agents or MCP servers into their coding harnesses - setting up the tools repo beside the domain, laying out the folders, bringing in skills they already have, adding or changing the MANIFEST Provisioning section, asking for the allow or deny decision, or updating provisioned tools (provision apply) after a change, an update or a git pull.
---

# Crystalline Provisioning

A domain can ship the tools its knowledge depends on - skills, commands, agents and MCP servers - into the person's coding harnesses. The person decides; you do the legwork and show each step first.

## Before you start

Call `list_domains` and take the domain's `path`. Stop and say why when the domain is virtual or when that path does not exist on the machine your shell sees: the server then runs elsewhere, and the person provisions with `crystalline provision` on their own machine. Call `provision` with `action: "status"`; if it lists no harness, the person runs `crystalline install <harness>` first. Call `origin_status`: if the domain has a team origin, choose the location from "Team domains" below. If `origin_status` is not among your tools, or refuses because collaboration is off, the domain is local; do not turn collaboration on.

## Choose where the tools live

For a local domain the tools live in a git repo beside the domain folder, by default `<domain-folder>-tools`, declared from the MANIFEST as `../<domain-folder>-tools/<kind>`. If the person names an existing repo, propose `git clone <url> <path>`; for a new one, propose `git init <path>`. Show the exact path and command, then stop: end your turn and run it only after the person says yes.

## Lay out the folders

Create one folder per kind, and only the kinds the person will use:

- `skills/<name>/SKILL.md` with frontmatter `name` equal to the folder name and a one-line `description`. Scripts and references inside the skill folder ship with it.
- `commands/<name>.md` or `commands/<namespace>/<name>.md`, frontmatter optional (`description`, `argument-hint`).
- `agents/<name>.md` with frontmatter `name` and `description`; the body is the agent's instructions. Write agents in markdown, never `x.md` beside `x.toml`.
- `mcps/<name>.json`, a JSON object with `name` and a `server` object: stdio as `command`, `args` and `env`, HTTP as `"type": "http"`, `url` and `headers`.

```json
{ "name": "tide-tables", "server": { "command": "tide-mcp", "args": ["--stdio"] } }
```

Skills reach all seven harnesses in the table below. Agents and MCP servers reach Claude Code, Codex and GitHub Copilot CLI only, and commands reach Claude Code and Codex only, so prefer a skill unless the person uses only those harnesses. An HTTP server that needs a header does not work in Codex. In the other harnesses the person registers MCP servers themselves. Write real files: symlinks and anything whose name starts with `.` never ship.

## Name things

Use lowercase letters, digits and hyphens. A name never starts with `.` and never contains `/`, `\` or `:`. Never use the `crystalline-` prefix: those names belong to `crystalline install`. Do not reuse a name another domain already ships. Do not put `ns/name.md` beside `ns-name.md` in commands.

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

To see what other domains already ship, read the `## Provisioning` section of each registered domain's MANIFEST (`read_engram` with `"identifier": "manifest"` per domain from `list_domains`) and list the declared skills folders. Skip candidates named `crystalline-*` or already present in one of those folders. Show the list with each description; the person chooses. Read every chosen skill before you copy it, scripts included, and say what each script runs. Copy, never move: once the person has confirmed the tools repo, copy each chosen skill into `<tools-repo>/skills/<name>/` with its folder name, before the MANIFEST step.

Before `allow`, tell the person what happens to each original. Crystalline writes skills into one folder per harness: `~/.claude/skills`, `~/.agents/skills`, the Copilot `skills` folder, `~/.kiro/skills` and `~/.qwen/skills`. An unchanged original in one of those becomes managed by Crystalline, and a later `deny` removes it; one they changed after the copy is left alone, and the repo copy does not install there. An original in any other folder stays, and a harness that reads both folders shows the skill twice; if the person wants one copy, they remove that original after `allow`.

To bring in an MCP server the person already registered, rewrite it as an `mcps` file and leave out every secret value (see "Never").

## Declare the folders in the MANIFEST

To see or change how a domain behaves (its sharing, generated indexes, MANIFEST sections or rule overrides), call configure with the domain.

Say which bullets you will write, one per kind, each `kind: path` relative to the MANIFEST. Then call `read_engram` with `"identifier": "manifest"` and the domain, and `edit_engram` with the `expected_checksum` from that read.

For a new section use `"operation": "append"` with `## Provisioning` and the bullets as content:

```json
{ "tool": "edit_engram", "arguments": { "domain": "harbor", "identifier": "manifest", "operation": "append",
  "content": "## Provisioning\n\n- skills: ../harbor-tools/skills\n- mcps: ../harbor-tools/mcps\n",
  "expected_checksum": "<from read_engram>" } }
```

For an existing section use `"operation": "replace_section"` with `"section": "## Provisioning"` and the bullets only, without the heading. Read `manifest_findings` on the receipt: fix every finding your edit caused before going on, and tell the person about the others.

If the receipt marks the edit as a draft, the domain reviews changes first: the section counts, and `allow` works, only after the draft is shared and merged. Tell the person, offer to share it with the `crystalline-collaboration` skill and ask for the decision only once it is merged.

After every MANIFEST edit that is not a draft, call `provision` `status` and compare the domain's count for each declared kind with what you laid out. A wrong path raises no finding; a zero there means that kind's path is wrong, so fix it before the decision.

## Ask for the decision

Summarize in one sentence what the domain would ship, with the counts per kind, and ask whether to allow or deny. Call `provision` only after the person answers:

```json
{ "tool": "provision", "arguments": { "action": "allow", "domain": "harbor" } }
```

A request to set up provisioning is not consent to allow it.

## Check the result

After every allow or deny, call `provision` `status` again. A harness row with `covered_by` set shows zeros; its skills are counted under the harness it names. Relay every notice from the apply, such as an MCP name already registered outside Crystalline (the person removes it first) or a command to run by hand. With a shell, also run `crystalline doctor`.

## Change or update later

Change artifacts in the tools repo: a hand edit in a harness folder is replaced on the next apply and kept as a `.bak` file. After an edit, or after a `git pull` you proposed and the person confirmed, call `provision` with `action: "apply"` (or run `crystalline provision`) right away: MCP servers change only then. Before a `deny`, say that it removes everything the domain shipped, adopted originals included.

## Team domains

Put the tools inside the domain folder (`skills: skills`, `mcps: mcps`): they travel with `share_changes` like any file and reach teammates with `update_domain`. After an `update_domain` that changed them, call `provision` `apply`.

Use `../` only when the person confirms the domain is a subfolder of its repository and the target stays inside that repository; ask, because no tool reports it. Never point `../` above the repository root: once merged, it breaks `update_domain` for every teammate.

A `../` folder is read from the team repository, not from your disk: it ships only after it is merged upstream, and `share_changes` does not carry it. Propose committing those files with git and a pull request.

A separate tools repo works only as a domain of its own, which each person subscribes or registers and declares in its own MANIFEST. Every teammate still decides allow or deny on their own machine. `crystalline doctor` shows whether the artifact mirror for a `../` folder is present.

## Never

- Never write a secret value (token, password, API key) into an `mcps` file or any artifact: the `server` object lands in clear in the harness config and in the repo. For a stdio server, tell the person which variable the server reads and where to set it, and leave the value out. An HTTP server that needs a secret header cannot ship in a working way: write no `mcps` file for it, and tell the person to register it in each harness themselves.
- Never ship a skill, command, agent or MCP server you have not read: it runs with the person's permissions.
- Never decide `allow` or `deny`, and never take a setup request as the decision.
- Never run `git clone`, `git init`, `git pull`, commit or push without showing the path and command and getting a yes.
- Never move a skill out of a harness folder; copy it.
- Never edit provisioned files inside a harness folder; edit the source in the tools repo.
- Never put provisioning text into the person's instruction files (CLAUDE.md, AGENTS.md, GEMINI.md, QWEN.md and the like); the MANIFEST section is the only declaration.

## Quick reference

- Any provisioning request -> `list_domains`, `provision` `status`, `origin_status`.
- New section -> `read_engram` `manifest`, `edit_engram` `append` with `expected_checksum`; existing -> `replace_section`, body only; then `provision` `status`.
- Draft receipt -> share and merge before the decision.
- Allow or deny -> check the counts, summarize, call `provision` only after the person answers.
- After an edit, a `git pull` or an `update_domain` -> `provision` `apply` at once.
- Team domain -> folders inside the domain; `../` only within the repository root.
