---
name: crystalline-collaboration
description: Use when working in a domain that has a team origin on GitHub - checking status at session start, updating before deep work, sharing new knowledge as a proposal, settling a conflict, or connecting a new teammate.
---

# Crystalline Collaboration

A team domain is an ordinary domain that also tracks a GitHub repository: the files on disk stay the source of truth on this machine, and an origin records which repository, subfolder and branch it follows. Call `origin_status` with no arguments to see which registered domains have a team origin and where each one stands; it lists only origin-connected domains, so an empty result means none of the current domains are shared this way.

## Session start in a shared domain

Before doing deep work in a domain with a team origin, call `origin_status` for that domain. If it reports the domain is behind, call `update_domain` to bring it up to date first: this merges the team's latest knowledge cleanly where possible and flags any real conflicts for `resolve_conflict`. Skipping this risks building on stale knowledge or creating an avoidable conflict later.

## Capture, then share deliberately

The capture loop itself does not change: follow `crystalline-capture` for what is worth writing down, searching before writing and editing over creating. Sharing is a separate, deliberate act on top of that loop. When a coherent unit of knowledge is ready - not mid-thought, not half-written - call `share_changes` with a meaningful title that describes what changed and why, then relay the returned review URL to the person you are working with. Review and merging happen on GitHub, by a person; the agent never merges its own proposal and never treats a share as done until it is relayed.

## Declined proposals

A declined proposal is a normal outcome, not an error: `origin_status` surfaces it with its URL. Read the feedback with the person, then either refine the local content and call `share_changes` again, or ask them to run `crystalline origin discard <domain> --proposal <n>` to abandon it and restore any untouched local files to their pre-share state - discarding is a CLI-only verb today, with no equivalent MCP tool for the agent to call directly.

## Conflict etiquette

A conflict means a teammate's merged knowledge and local knowledge disagree on the same file, and Crystalline left the local file untouched rather than guessing. `origin_status` lists each conflict's path and kind. Read the local side with `read_engram` before deciding; Crystalline does not expose the pre-merge upstream copy through a tool, so if you need to see the team's exact wording rather than just its existence, ask the person to check the merged change on GitHub. Settle the conflict with `resolve_conflict`: `mine` keeps the local version, `theirs` takes the team's version, `merged` takes content you supply after reconciling both sides. Never hand-edit conflict state directly - always go through `resolve_conflict`. After resolving with `mine`, the file counts as an ordinary local change again and can be shared like any other.

## Onboarding a colleague, no terminal required

A non-engineer can go from nothing to a working team domain entirely through chat. Call `configure` with `connect: "github"`: it returns a short code and a verification URL. Relay both and ask them to open the URL in a browser where they are already signed in to GitHub and enter the code - no git, no SSH key and no token to paste. Once connected, call `add_domain` with the team's repository (and a subfolder or branch if it needs one); this registers the domain, downloads its knowledge and makes it searchable immediately. Knowledge that already exists locally is no obstacle: pointing `add_domain` at a non-empty folder, or at a registered domain that has no origin yet, connects it in place - local files are never overwritten, and any that differ from the repository simply become local changes to share with `share_changes` or reconcile with `update_domain`. Git, `gh` and local clones are never involved anywhere in this flow: Crystalline talks to the GitHub API directly and hides the plumbing.

## Enabling collaboration

Team domains are off by default. Turn them on with `configure` (`set: { "github.enabled": "true" }`), or from the CLI with `crystalline config set github.enabled true`. Until this is on, only `configure` itself is visible among the collaboration tools; once it is enabled, `share_changes`, `update_domain`, `origin_status` and `resolve_conflict` become available too, immediately in a harness that refreshes its tool list live, otherwise from the next session. `add_domain` is not one of these: it creates domains of every kind (local folder, virtual, team) and is always available on a writable instance, so you can capture knowledge with no GitHub at all; only its team-domain branch (passing a `repo`) needs `github.enabled` on first. A container operator can set `CRYSTALLINE_GITHUB_ENABLED=true` (and the other `CRYSTALLINE_*` variables, including `CRYSTALLINE_GITHUB_TOKEN` for a headless machine's GitHub identity) instead of editing config.
