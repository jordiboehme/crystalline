---
name: crystalline-capture
description: Use when durable knowledge is learned while working, when the user asks to update a Crystalline domain, or before writing or editing an engram so it is deduplicated, well-formed and linked correctly.
---

# Crystalline Capture

Capturing what you learn while you work, as engrams, is the core of Crystalline. Treat it as a normal byproduct of the task, not a separate chore to remember at the end.

Some deployments are read-only: if the server instructions or an injected prompt say this deployment's knowledge is read-only and curated externally, stand down on capture. The write tools are not exposed there and this whole skill does not apply; search and read instead.

## What to capture

Capture durable knowledge: decisions, confirmed facts, repeatable patterns, gotchas, conventions, lessons learned the hard way, known risks and explicitly speculative ideas or proposals. Do not capture transient debugging steps, one-off scratch state, or knowledge so narrow it only ever matters to the current session.

If the user did not explicitly ask for this, ask first:

> I noticed `<one-sentence insight>`. Should I capture this in `<domain>` as `[<category>]`?

## Always name the domain

Every write requires an explicit `domain` - there is no default domain for writes, by design, so knowledge never lands in the wrong place by accident. If it is not obvious which domain owns a new piece of knowledge, ask the user rather than guessing; do not silently pick the domain that happens to already be open. Use the `crystalline-routing` skill if you need help identifying the right domain first.

## Search before you write

Before creating anything, search for it - the same `search_engrams` call `crystalline-routing` already shows, scoped or broad. A hit can be a whole engram or a single observation inside one (`search_engrams` returns both kinds in the same result set, an observation hit carries its source line) - check both before deciding there is nothing to update. When the knowledge could plausibly live in another domain too, run one more sweep with `domains` omitted.

## Edit over create

Prefer refining an existing engram over starting a new one for the same topic, as long as the new knowledge shares the same validity state as what is already there:

```json
{
  "tool": "edit_engram",
  "arguments": {
    "identifier": "retry-queue-gotcha",
    "domain": "payments",
    "operation": "append",
    "content": "- [lesson] Doubling the backoff window did not help; the fix was raising the dead-letter TTL #payments"
  }
}
```

`operation` is one of `append`, `prepend`, `find_replace`, `replace_section`, `insert_before_section`, `insert_after_section`. Address sections by heading path, for example `## API > ### Auth`; `replace_section` keeps deeper subsections in place unless `include_subsections` is set, so a targeted rewrite never silently drops content nested under the heading you replaced.

Create a new engram only when no existing one owns the topic, or the existing owner is in a different validity state (see below). Keep one topic per engram - do not fold an unrelated second topic into an engram just because it is convenient.

## Writing a new engram

```json
{
  "tool": "write_engram",
  "arguments": {
    "domain": "payments",
    "title": "Retry queue gotcha",
    "type": "engram",
    "tags": ["gotcha", "payments"],
    "content": "- [gotcha] The retry queue drops jobs older than 24h #payments\n- depends_on [[Retry Queue Architecture]]"
  }
}
```

`permalink`, `status` (defaults to `current`), `recorded_at` and `timestamp` are filled in for you; `valid_from`/`valid_to` are never auto-set. Recommended `type` values: `engram`, `guide`, `decision`, `architecture`, `runbook`, `reference`. Recommended `status` values: `current`, `implemented`, `draft`, `proposed`, `idea`, `poc`, `deprecated`, `superseded`, `archived`, `legacy` - this is guidance so you can tell an idea or draft apart from current fact, not a fixed enum a write is rejected for.

## Observation categories

Pick the most precise bullet category for `- [category] content #tag`:

- `[decision]` - a choice that was made
- `[fact]` - verified current state
- `[pattern]` - a repeatable approach
- `[gotcha]` - a non-obvious pitfall
- `[convention]` - a team agreement
- `[lesson]` - learned from experience, often the hard way
- `[risk]` - a known concern
- `[insight]` - a realization that changes understanding
- `[idea]`, `[proposal]`, `[poc]` - speculative or draft content; never mark speculation as `[fact]` or `[decision]`

Relations connect engrams to each other: `- depends_on [[Other Engram]]`, or a quoted multi-word type like `- "relates to" [[Other Engram]]`.

## Temporal fields

`valid_from`/`valid_to` are optional. Absence is the normal case and it means unbounded: no `valid_from` means the engram has always been valid, no `valid_to` means it is valid forever. Set them only when validity is genuinely bounded - a policy that changes on a known date, a temporary workaround with a known expiry. Never write a sentinel far-future date to mean "forever"; just omit the field.

## Superseding instead of contradicting

Do not let an engram hold both current and outdated guidance at once. When new knowledge replaces old knowledge:

1. Write or edit the current engram with `status: current`.
2. Edit the old engram: set its `status` to `superseded` (or `deprecated`).
3. Add a `- superseded_by [[New Engram]]` relation on the old engram (and, optionally, `- supersedes [[Old Engram]]` on the new one).

## Confirm before destroying

Always confirm with the user before calling `delete_engram` or `move_engram` - describe what will be removed or relocated and wait for a yes. Prefer setting `status` to `deprecated` or `superseded` over deleting when the history is still worth keeping; `move_engram` on a cross-domain move rewrites inbound bare links to the domain-prefixed form automatically unless `update_links` is set to `false`.
