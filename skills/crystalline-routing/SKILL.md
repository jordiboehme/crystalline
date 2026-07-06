---
name: crystalline-routing
description: Use when deciding which Crystalline domain(s) to search or read for a task, especially at session start, before reading a MANIFEST directly, when answering a broad question across domains, or when a "what is true now" question needs temporal filtering.
---

# Crystalline Routing

Crystalline organizes what you have been taught into Domains, each with a `MANIFEST.md` describing its scope. Before you act on a task, know which domain(s) it lives in and search rather than guessing from memory or pre-trained knowledge.

## Start from the routing block

At session start you are handed a routing block - injected as a session prompt in some harnesses, served as the MCP server's own instructions in others - with one routing line per registered domain summarizing when to use it, built from its `MANIFEST.md` `## When to Use` bullets, plus the crystalline MCP tool names (`search_engrams`, `write_engram` and the rest) those domains route through. Treat each routing line as a targeting aid, not a complete catalog - a domain that looks unrelated at a glance may still hold the answer.

Either way, `list_domains` with `include_routing: true` re-fetches the same index mid-session.

## Decide scope first

1. **Narrow** - the task clearly belongs to one product, service or team, and a domain obviously owns it. Search that domain directly.
2. **Broad or unclear** - cross-cutting questions ("how do we generally handle X"), unclear ownership, or comparisons across domains. Sweep everything first, then narrow.

### Narrow: target a domain

```json
{
  "tool": "search_engrams",
  "arguments": { "query": "retry queue backoff", "domains": ["payments"] }
}
```

Read the strongest hit with `read_engram` before writing anything new.

### Broad: sweep, then narrow

Omit `domains` entirely - `search_engrams` defaults to every registered domain, and each hit is labelled with the domain it came from:

```json
{
  "tool": "search_engrams",
  "arguments": { "query": "incident postmortem process" }
}
```

Do not pre-filter to the domains you recognize from the routing block; a domain returning no hits for one phrasing does not mean the knowledge is not captured there under different words. If hits span several domains and the guidance conflicts, say so explicitly and name which domain each answer came from rather than picking one silently.

## Temporal filtering: "what is true now"

`status: current` is the primary, reliable signal for currency - filter on it directly:

```json
{
  "tool": "search_engrams",
  "arguments": { "query": "deployment process", "status": "current" }
}
```

Absence of `valid_from`/`valid_to` means unbounded validity (the write-side rule lives in `crystalline-capture`), so a `metadata_filters` range check on those fields excludes engrams that never set them: a comparison like `{"valid_to": {"$gt": "2026-07-02"}}` is a strict SQL predicate that skips null rows, the opposite of what "absence means unbounded" implies. Use such a filter only for an explicit, bounded validity window - for example, to find what was true on a past date:

```json
{
  "tool": "search_engrams",
  "arguments": {
    "query": "pricing",
    "metadata_filters": { "valid_from": { "$lte": "2026-01-01" }, "valid_to": { "$gt": "2026-01-01" } }
  }
}
```

For an ordinary present-day question, `status: current` alone is both correct and sufficient. When history matters - how something changed, why a decision was replaced - include `deprecated`, `superseded`, `draft`, `idea` and `poc` engrams too and narrate each one's status rather than presenting it as current fact.

## Read a MANIFEST only when needed

Read a domain's `MANIFEST.md` (via `read_engram` or `browse_domain`) only when:

- the routing line for that domain is too ambiguous to act on,
- the task is about the domain's own structure or conventions, or
- you are about to write or reorganize engrams inside it.

Otherwise, search first. Reading every MANIFEST up front burns context for no benefit once the routing block already exists.

## Explore the neighbourhood with build_context

Once you have an anchor engram, follow its relations and links (including across domains) instead of re-searching piecemeal:

```json
{
  "tool": "build_context",
  "arguments": { "anchor": "crystalline://payments/retry-queue-gotcha", "depth": 2 }
}
```

A `/*` suffix on the anchor globs a permalink prefix, useful for pulling in an entire topic's engrams at once, for example `crystalline://payments/retry-queue/*`.

## Quick reference

- One domain obviously owns it -> `search_engrams` with `domains: ["that-domain"]`.
- Broad, unclear or cross-domain -> `search_engrams` with `domains` omitted (an all-domain sweep).
- "What is true now" -> `status: "current"`; add `valid_from`/`valid_to` filters only for a specific bounded-in-time question.
- Need the shape of a domain, not its content -> `browse_domain` or its `MANIFEST.md`.
- Need what surrounds a known engram -> `build_context`.
- Before writing anything, switch to the `crystalline-capture` skill.
- Domain has a team origin (`origin_status` says so) -> switch to the `crystalline-collaboration` skill for status, sharing and conflicts.
