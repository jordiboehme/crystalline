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

1. **Narrow** - the task clearly belongs to one product, service or team, and a domain obviously owns it. Search that domain directly. A topic match without a named domain is still narrow: if the question's own words match what a routing line describes, scope to that domain rather than sweeping.
2. **Broad or unclear** - cross-cutting questions ("how do we generally handle X"), unclear ownership, or comparisons across domains. Sweep everything first, then narrow.

An ambiguous term is not grounds to ask the user for clarification before searching - a search is cheap and usually resolves the ambiguity by returning (or failing to return) relevant hits. Ask only after a search came back empty or clearly unrelated.

### Narrow: target a domain

```json
{
  "tool": "search_engrams",
  "arguments": { "query": "retry queue backoff", "domains": ["payments"] }
}
```

When the question is also about the present-day state, put `domains` and `status: "current"` in the same call - one round trip instead of a search plus a filtered retry:

```json
{
  "tool": "search_engrams",
  "arguments": { "query": "standard plan price", "domains": ["policies"], "status": "current" }
}
```

If a hit's snippet already states the fact you need, answering from it is fine. Reserve `read_engram` for when you need the full body, its relations or an exact quote - and pass the hit's permalink as `identifier` (bare or as a `crystalline://domain/permalink` URL); there is no `permalink` argument on `read_engram`.

### Broad: sweep, then narrow

Omit `domains` entirely - `search_engrams` defaults to every registered domain, and each hit is labelled with the domain it came from:

```json
{
  "tool": "search_engrams",
  "arguments": { "query": "incident postmortem process" }
}
```

Do not pre-filter to the domains you recognize from the routing block; a domain returning no hits for one phrasing does not mean the knowledge is not captured there under different words. If hits span several domains and the guidance conflicts, say so explicitly and name which domain each answer came from rather than picking one silently.

Zero hits is not the only signal to broaden. A scoped search can return plausible hits that do not actually answer the question - common when the question's subject belongs to one domain but the question itself is about ownership, escalation or roles, which another domain owns (who gets paged about refunds is a people question, not a payments one). When the obvious subject-matter domain keeps surfacing near-misses after a rephrasing or two, stop rephrasing inside it and run one unscoped sweep.

## Temporal filtering: "what is true now"

`status: current` is the primary, reliable signal for currency - filter on it directly:

```json
{
  "tool": "search_engrams",
  "arguments": { "query": "deployment process", "status": "current" }
}
```

Absence of `valid_from`/`valid_to` means unbounded validity (the write-side rule lives in `crystalline-capture`), so a `metadata_filters` range check on those fields excludes engrams that never set them: a comparison like `{"valid_to": {"$gt": "2026-07-02"}}` is a strict SQL predicate that skips null rows, the opposite of what "absence means unbounded" implies. Use such a filter only for an explicit, bounded validity window. For "what was true on or in <past date>" questions that window filter belongs on the first search, not in a retry:

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

Classify the question shape before dropping the `current` filter:

- **Binary adoption questions** - "are we running on X", "did we adopt Y", "is Z live". The yes/no itself is in doubt, so search without a status filter and read each hit's own `status`; a `current`-only filter would hide the draft or superseded engram that answers "no". Name the status in the answer ("proposed but never marked current").
- **Attribute questions** - "how long does X live", "how much does Y cost", "how many stages". The thing exists; only its present value is unknown. Keep `status: "current"` even when the subject sounds operational.

Two practical notes on window filters: `metadata_filters` must be a JSON object, not a JSON-encoded string - a stringified object is rejected with an invalid-input error. And when a window filter is not getting traction, the `supersedes`/`superseded_by` relations plus each hit's `status` tell the same temporal story: search by keyword (topic or year) and read which engram covers the period asked about.

## Read a MANIFEST only when needed

Read a domain's `MANIFEST.md` (via `read_engram` or `browse_domain`) only when:

- the routing line for that domain is too ambiguous to act on,
- the task is about the domain's own structure or conventions, or
- you are about to write or reorganize engrams inside it.

Structure questions ("what does domain X cover", "what is domain X for", "which domain owns Y and what else does it hold") are the second case: they ask for the MANIFEST's own content, so the MANIFEST read is your first tool call, made before drafting any answer text. The routing line is a compressed derivative of the MANIFEST, not the source - answering from it and treating the read as optional confirmation is exactly the failure this rule prevents, however complete the routing line looks.

Address the MANIFEST by its permalink, usually `manifest` - `{"identifier": "manifest", "domain": "x"}` or the `crystalline://x/manifest` URL. The filename `MANIFEST.md` is not an identifier; when unsure, `browse_domain` lists the real permalink.

Otherwise, search first. Reading every MANIFEST up front burns context for no benefit once the routing block already exists.

## Knowledge questions are not codebase questions

Operational and status questions ("are we on X", "do we use Y", "how long is Z kept") are knowledge-base questions: reach for `search_engrams` first, not for file listings, code search or unrelated skills. A tool erroring as unavailable, or a working directory that is not a repository, is a signal that the fact lives in a Crystalline domain - stop hopping tools and search.

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

- One domain obviously owns it (even if unnamed) -> `search_engrams` with `domains: ["that-domain"]`.
- Broad, unclear or cross-domain -> `search_engrams` with `domains` omitted (an all-domain sweep).
- Scoped hits keep missing the point -> one unscoped sweep; the question's owner may not be the subject's owner.
- "What is true now" -> `status: "current"`; add `valid_from`/`valid_to` filters only for a specific bounded-in-time question.
- "Did we adopt X" -> search without a status filter and narrate each hit's status.
- Need the shape of a domain, not its content -> `browse_domain` or its `MANIFEST.md`.
- Need what surrounds a known engram -> `build_context`.
- Before writing anything, switch to the `crystalline-capture` skill.
- Domain has a team origin (`origin_status` says so) -> switch to the `crystalline-collaboration` skill for status, sharing and conflicts.
