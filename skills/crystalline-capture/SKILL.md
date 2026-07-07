---
name: crystalline-capture
description: Use when durable knowledge is learned while working, when the user asks to update a Crystalline domain, or before writing or editing an engram so it is deduplicated, well-formed and linked correctly.
---

# Crystalline Capture

Capturing what you learn while you work, as engrams, is the core of Crystalline. Treat it as a normal byproduct of the task, not a separate chore to remember at the end. A session-end reminder may ask you to review the conversation for uncaptured learnings - handle it with this skill and finish quietly when nothing qualifies.

Some deployments are read-only: if the server instructions or an injected prompt say this deployment's knowledge is read-only and curated externally, stand down on capture. The write tools are not exposed there and this whole skill does not apply; search and read instead.

## What to capture

Capture durable knowledge: decisions, confirmed facts, repeatable patterns, gotchas, conventions, lessons learned the hard way, known risks and explicitly speculative ideas or proposals. Do not capture transient debugging steps, one-off scratch state, or knowledge so narrow it only ever matters to the current session.

If the user did not explicitly ask for this, ask first:

> I noticed `<one-sentence insight>`. Should I capture this in `<domain>` as `[<category>]`?

Two boundaries keep this quiet and useful. An obviously transient aside - a tool complaint, a scratch note, a work-in-progress caveat - does not deserve the ask-first prompt; let it pass without comment. And a terse imperative ("store X", "capture this: Y") is a complete capture request, not a fragment to clarify: the clause after the verb is the content and an engram named in it ("link it to the retry queue engram") is a relation target to find by title. When one message mixes throwaway scratch with a real question, skipping capture for the scratch half never excuses skipping the search before answering the question half.

## Always name the domain

Every write requires an explicit `domain` - there is no default domain for writes, by design, so knowledge never lands in the wrong place by accident. If it is not obvious which domain owns a new piece of knowledge, ask the user rather than guessing; do not silently pick the domain that happens to already be open. Use the `crystalline-routing` skill if you need help identifying the right domain first.

The domain of the surrounding story is not automatically the domain of the knowledge: a rotation detail learned during a payments incident is people knowledge, not payments knowledge. When the topic category (who owns it, what process it is) differs from the context it surfaced in, sweep with `domains` omitted and let the hits name the true owner.

## Search before you write

Before creating anything, search for it - the same `search_engrams` call `crystalline-routing` already shows, scoped or broad. A hit can be a whole engram or a single observation inside one (`search_engrams` returns both kinds in the same result set, an observation hit carries its source line) - check both before deciding there is nothing to update. When the knowledge could plausibly live in another domain too, run one more sweep with `domains` omitted.

Treat this as a hard gate: a `search_engrams` call comes before the first `write_engram` or `edit_engram` on a topic, no exceptions, and loading a tool schema does not count as searching. When a hit looks like the owner, `read_engram` it before editing - a snippet is not enough to judge fit or write a correct edit - and pass the returned checksum as `expected_checksum` on the edit so a concurrent change is rejected instead of overwritten. Both tools take the permalink as `identifier`; keep it bare when `domain` is passed alongside, since a domain-prefixed identifier plus a separate `domain` argument does not resolve.

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

`operation` is one of `append`, `prepend`, `find_replace`, `replace_section`, `insert_before_section`, `insert_after_section`. Address sections by heading path, for example `## API > ### Auth`; `replace_section` needs a real, non-empty `section` heading (use `find_replace` when there is no clean heading to target) and keeps deeper subsections in place unless `include_subsections` is set, so a targeted rewrite never silently drops content nested under the heading you replaced.

Create a new engram only when no existing one owns the topic, or the existing owner is in a different validity state (see below). Keep one topic per engram - do not fold an unrelated second topic into an engram just because it is convenient.

The test for edit versus supersede: does the new information make a fact in the owner engram false going forward ("changed", "replaced", "no longer", "instead of")? Then it is a supersession however small the diff looks, even a one-word value swap - follow the recipe below instead of rewriting the old engram's content in place. A pure addition or clarification that contradicts nothing is a normal edit.

## Writing a new engram

```json
{
  "tool": "write_engram",
  "arguments": {
    "domain": "payments",
    "title": "Retry queue gotcha",
    "type": "engram",
    "tags": ["gotcha", "payments"],
    "content": "- [gotcha] The retry queue drops jobs older than 24h #payments\n- [fact] Confirmed during the March incident postmortem #payments\n- depends_on [[Retry Queue Architecture]]"
  }
}
```

An engram needs at least 3 non-blank content lines to pass verification - a lone bullet is rejected as too thin. When the user hands you a single sentence, pad honestly rather than asking for more or refusing to write: one bullet for the fact in the user's own words, one for provenance (how and when it was learned) and one for scope or implication. An inferred consequence is fine; an invented specific is not.

**Before every write or edit, check the exact `content` value you are about to send**: count its non-blank lines (fewer than 3 on a new engram fails) and confirm the bullets are separated by real newline characters. The most common failure is the two printable characters backslash and n between bullets - the string looks multi-line in your draft but lands as one long line and is rejected as thin. If you cannot tell by eye, rebuild the string bullet by bullet.

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

Set them through the `metadata` argument as an object; a bound stated only in the content prose does not bound anything for search or supersede logic:

```json
{
  "tool": "write_engram",
  "arguments": {
    "domain": "payments",
    "title": "Manual chargeback review workaround",
    "tags": ["workaround", "payments"],
    "metadata": { "valid_to": "2026-11-30" },
    "content": "A temporary process until automation lands.\n\n- [fact] Chargebacks are reviewed manually until 2026-11-30 #payments\n- [risk] Manual review adds a day of latency #payments"
  }
}
```

They are not top-level write arguments - an unknown argument is silently dropped, so a bound passed that way vanishes without an error. After a bounded write, read the engram back and confirm the fields landed before reporting the window as set.

## Superseding instead of contradicting

Do not let an engram hold both current and outdated guidance at once. When new knowledge replaces old knowledge:

1. Write the replacement as its own new engram with `status: current` - do not rewrite the old engram's factual content in place, which skips step 2 and leaves the outdated fact searchable as current.
2. Edit the old engram: set its `status` to `superseded` (or `deprecated`) with a `find_replace` targeting the frontmatter line, `find_text: "status: current"` and `content: "status: superseded"`. There is no status argument on `edit_engram`; passing one is silently ignored.
3. Add a `- superseded_by [[New Engram]]` relation on the old engram (and, optionally, `- supersedes [[Old Engram]]` on the new one).

Read the old engram back afterwards and confirm its status actually changed before reporting the supersession done.

## Confirm before destroying

Always confirm with the user before calling `delete_engram` or `move_engram` - describe what will be removed or relocated and wait for a yes. Prefer setting `status` to `deprecated` or `superseded` over deleting when the history is still worth keeping; `move_engram` on a cross-domain move rewrites inbound bare links to the domain-prefixed form automatically unless `update_links` is set to `false`.
