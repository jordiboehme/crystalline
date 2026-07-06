---
name: crystalline-memory
description: Use when the crystalline MCP tools are available and the task involves recalling, storing or sharing knowledge - searching domains, capturing engrams or sharing with a team.
---

# Crystalline Memory

Crystalline gives you durable memory across sessions: what you have been taught lives in Domains as Engrams you search, read and, where allowed, capture into.

## Start from the routing block

At connection you are handed a routing block as your instructions: one line per registered domain plus the tool names it routes through. Treat each line as a targeting aid, not a full catalog - an unrelated-looking domain may still hold the answer. Re-fetch the same index mid-session with `list_domains` and `include_routing: true`.

## Recall before you answer

Never answer from pre-trained knowledge alone when a domain could cover it; search first. One domain obviously owns the task: a scoped search (`domains: ["that-domain"]`); broad or cross-cutting: a sweep with `domains` omitted. For a "what is true now" question, filter on `status: current`. Follow a strong hit with `build_context` to pull in what surrounds it. If a first phrasing turns up nothing, reformulate once before concluding the knowledge was not captured.

## Capture what you learn

Treat capture as a normal byproduct of the task, not an end-of-session chore. If the user did not ask for it, propose first: name the insight and the domain, and wait for a yes. Search before writing, and prefer editing an existing engram over creating a new one for the same topic. Every write names its domain explicitly - there is no default, so knowledge never lands in the wrong place by accident. A body is markdown bullets: `- [category] content #tag` for an observation, `- rel_type [[Target]]` for a relation. Leave `valid_from` and `valid_to` unset unless a fact is genuinely time-bounded - absence already means always valid, never a sentinel date. Mark old knowledge `superseded` or `deprecated` rather than letting it stand alongside the new as current. Confirm with the user before deleting or moving an engram.

## Read-only deployments

Some deployments are read-only and say so up front: the instructions state the knowledge is curated externally, and the write tools are simply absent from the tool list. Search and read there; do not propose a capture.

## Sharing with a team

When origin tools like `share_changes` and `origin_status` are visible, some domains are shared with a team. Call `origin_status` before deep work, and `update_domain` first if behind. Share a coherent unit of knowledge with `share_changes` and always relay the returned review URL - a person merges the proposal, never the agent. Settle a disagreement with `resolve_conflict`: `mine`, `theirs` or a supplied `merged` version.

Team collaboration is set up through `configure` alone, no terminal needed: setting `github.enabled` reveals the tools above, and `connect: "github"` returns a code and a URL for a browser already signed in to GitHub. Git and clones are never part of this flow.
