---
name: crystalline-capture
description: Use when durable knowledge is learned while working, when the user asks to update a Crystalline domain, or before writing or editing an engram so it is deduplicated, well-formed and linked correctly.
---

# Crystalline Capture

Capturing what you learn while you work, as engrams, is the core of Crystalline. Treat it as a normal byproduct of the task, not a separate chore to remember at the end. A session-end reminder may ask you to review the conversation for uncaptured learnings - handle it with this skill and finish quietly when nothing qualifies. That reminder sometimes carries a maintenance ask alongside it, and when it names focus domains, pass exactly those to `evolve_engrams` instead of sweeping everything. It can also carry a sharing ask counting work a team domain has not sent to the team: that one belongs to `crystalline-collaboration` - propose `share_changes` for the domain it names and wait for a yes, because sharing publishes somebody's work for review.

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

Before creating anything, search for it - the same `search_engrams` call `crystalline-routing` already shows, scoped or broad. A hit can be a whole engram or a single observation inside one (`search_engrams` returns both kinds in the same result set, an observation hit carries its source line) - check both before deciding there is nothing to update. Every hit also carries its engram's tags, so each search shows the vocabulary already in use. When the knowledge could plausibly live in another domain too, run one more sweep with `domains` omitted.

Treat this as a hard gate: a `search_engrams` call comes before the first `write_engram` or `edit_engram` on a topic, no exceptions, and loading a tool schema does not count as searching. When a hit looks like the owner, `read_engram` it before editing - a snippet is not enough to judge fit or write a correct edit - and pass the returned checksum as `expected_checksum` on the edit so a concurrent change is rejected instead of overwritten. Both tools take the permalink as `identifier`; keep it bare - an identifier without the `crystalline://` scheme is always domain-relative, so a domain-prefixed identifier never resolves.

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

`operation` is one of `append`, `prepend`, `find_replace`, `replace_section`, `insert_before_section`, `insert_after_section`, `set_frontmatter`. The first six edit the body; `set_frontmatter` assigns one lifecycle frontmatter field by `key` and `value` instead of matching text, which is how a status flip, a validity window, a review date or a verification is written. The settable keys are `status`, `valid_from`, `valid_to`, `stale_after`, `source_date`, `salience` and `verified`, and nothing else: an omitted `value` removes the field (`status` is the one key that cannot be removed, since every engram needs one), the four date keys take a plain ISO date (YYYY-MM-DD), `salience` takes a number from 0 to 10 and `verified` never removes - it stamps `{ by, at }` with the current instant, taking `value` as the verifying actor and falling back to your own identity when `value` is omitted. Address sections by heading path, for example `## API > ### Auth`; `replace_section` needs a real, non-empty `section` heading (use `find_replace` when there is no clean heading to target) and keeps deeper subsections in place unless `include_subsections` is set, so a targeted rewrite never silently drops content nested under the heading you replaced.

Create a new engram only when no existing one owns the topic, or the existing owner is in a different validity state (see below). Keep one topic per engram - do not fold an unrelated second topic into an engram just because it is convenient.

The test for edit versus supersede: does the new information make a fact in the owner engram false going forward ("changed", "replaced", "no longer", "instead of")? Then it is a supersession however small the diff looks, even a one-word value swap - follow the recipe below instead of rewriting the old engram's content in place. A pure addition or clarification that contradicts nothing is a normal edit.

Reading is also checking. When an engram you read states something the session now knows is false, or two hits disagree with each other, surface the conflict and propose the fix - reconcile in place or supersede per the test above - rather than appending the new fact beside the old. On a `find_replace` reconciliation, pass `expected_replacements` so a value appearing more times than you counted fails the edit instead of rewriting the wrong occurrence.

## Reconcile in place, not as an append log

An engram is what is true now, including what is now known to be outdated - it is never a change log. When re-ingesting a source or updating a topic, edit the body until it reads as a current-state description again: replace changed facts where they stand, insert new facts into the sections they belong in and delete what the source no longer supports. Never add a dated update section - `## Update - <date>`, `## Re-verification - <date>`, `## Refresh`, `## Change History`, `## Delta since <date>` or any sibling of these - and never append `- [fact] As of <date> X is Y` when the body already states X is Y: either the body needs updating or the observation is redundant. The version layer (git, a team domain's origin history) carries how knowledge changed; a body that reads as running commentary is wrong even when every entry was true as written.

"Checked the source, nothing changed" is one frontmatter field, not a heading: record it with `set_frontmatter` and `key: "verified"`, leaving `value` out so it stamps `verified: { by: <you>, at: <now> }` for you, plus a second call with `key: "source_date"` when the source is versioned. Your entry replaces your own previous one and leaves other actors' entries standing, so the trust record stays a history instead of growing a line per check; an older engram's legacy `last_verified: <date>` is still read as a verification, so a new `verified` entry beside it is enough. Neither touches `valid_from`/`valid_to` - verification does not bound validity. When the knowledge has a shelf life, `stale_after: <date>` (legacy spelling `review_after`) says when to check again.

Outdated knowledge earns its place only when the outdatedness is the point: a deprecated approach kept so it is not recommended again, a rejected alternative kept so it is not re-litigated, a documented anti-pattern. Mark it so it cannot read as current - `status: deprecated` or `superseded` on a whole engram, a `[lesson]` or `[gotcha]` bullet naming why something was retired inside a live one. The recognized retirement set is `deprecated`, `superseded`, `archived` or `legacy` - any of the four softly fades the engram in search ranking, while a free-form status like "obsolete" keeps it ranking at full strength.

Engrams whose topic is inherently historical carry history as structured current content, still not as an append log: a registry row holds its current status and one last-checked value, not a per-check trail; a decision engram tracks its current lifecycle status and is superseded when replaced; a meeting note records one meeting on one date - a new meeting gets a new engram, not an update block on the old one.

## Reuse the vocabulary

Before coining a new tag, observation category or relation type, call `vocabulary` (scoped with `domain`) and reuse an existing term rather than a near-synonym. A `clusters` entry flags near-duplicate tags that drifted apart - surface it to the user rather than acting; `crystalline tags rename` and `tags merge` are CLI cleanups they can run.

A domain MANIFEST may carry a `## Tag Aliases` section of `- old-name -> canonical-name` bullets; searches fold an aliased tag to its canonical form both directions, and `tags merge` records the alias. When the same concept appears under two tags, you MAY propose recording an alias - edit that section with `edit_engram` only after the user agrees, never as silent hygiene.

## Writing a new engram

```json
{
  "tool": "write_engram",
  "arguments": {
    "domain": "payments",
    "folder": "retry-queue",
    "title": "Retry queue gotcha",
    "type": "engram",
    "tags": ["gotcha", "payments"],
    "content": "- [gotcha] The retry queue drops jobs older than 24h #payments\n- [fact] Confirmed during the March incident postmortem #payments\n- depends_on [[Retry Queue Architecture]]"
  }
}
```

An engram needs at least 3 non-blank content lines to pass verification - a lone bullet is rejected as too thin. When the user hands you a single sentence, pad honestly rather than asking for more or refusing to write: one bullet for the fact in the user's own words, one for provenance (how and when it was learned) and one for scope or implication. An inferred consequence is fine; an invented specific is not.

**Before every write or edit, check the exact `content` value you are about to send**: count its non-blank lines (fewer than 3 on a new engram fails) and confirm the bullets are separated by real newline characters. The most common failure is the two printable characters backslash and n between bullets - the string looks multi-line in your draft but lands as one long line and is rejected as thin. If you cannot tell by eye, rebuild the string bullet by bullet.

`permalink`, `status` (defaults to `stable`), `recorded_at` and `generated` (who wrote it and when) are filled in for you; `valid_from`/`valid_to` are never auto-set. Recommended `type` values: `engram`, `guide`, `decision`, `architecture`, `runbook`, `reference`. Recommended `status` values: `stable` (the OKF word for knowledge that holds now; `current` is the older spelling of the same state and a filter on either matches both), `implemented`, `draft`, `proposed`, `idea`, `poc`, `deprecated`, `superseded`, `archived`, `legacy` - this is guidance so you can tell an idea or draft apart from current fact, not a fixed enum a write is rejected for.

The optional `folder` files the engram where its topic's neighbours already live, so check the domain's layout first and `browse_domain` it when the domain is unfamiliar. Start a subfolder once a topic cluster is forming; a singleton stays fine at the root. `index.md` and `log.md` are reserved names Crystalline maintains itself, so never write an engram that would take one. The folder becomes the permalink prefix `build_context` globs as `crystalline://domain/folder/*`.

When the body shows an attachment image, the target may carry a comma-separated formatting fragment - `left`, `right`, `center`, `full` and `w=NN` or `w=NN%`, as in `![Chart](assets/chart.png#right,w=50%)` - which Fluid honors and plain markdown viewers simply ignore.

Exceptionally valuable knowledge - a hard-won debugging insight, the decision that keeps paying off - can carry a numeric `salience` key (0 to 10) in `metadata`. Hybrid search adds a small bounded lift for it, so a salient engram ranks above equally relevant unmarked ones while relevance still dominates and nothing is ever filtered out by it. Most engrams need none; reserve it for knowledge that clearly outranks its neighbours, and when an engram later proves to be the key to a task, raise its salience with `set_frontmatter`, `key: "salience"` and the new number as `value`.

## Splitting a large capture

A transcript, a research document or a long specification splits by granularity, not by topic - the one sanctioned exception to one topic per engram. Write the distilled summary as a normal engram in the topic's usual folder, carrying the tags and any `salience`, and keep the full text as a `type: source` engram in a `sources/` subfolder. Link the pair both ways: `- summarizes [[Full Document]]` on the summary, `- summarized_by [[Summary]]` on the source. A full text over the verify token budget (2500 tokens by default) splits into sequential part engrams in the same folder, each linked back the same way. Verbatim retention is legitimate only alongside a distilled summary, never instead of one: the summary is what most recalls read, and the full text is pulled into context only when the summary falls short.

## Observation categories

Pick the most precise bullet category for `- [category] content #tag`:

- `[decision]` - a choice that was made
- `[fact]` - verified current state; date it only when the date is part of the fact ("accepted 2026-07-02"), never as as-of hedging
- `[pattern]` - a repeatable approach
- `[gotcha]` - a non-obvious pitfall
- `[convention]` - a team agreement
- `[lesson]` - learned from experience, often the hard way
- `[risk]` - a known concern
- `[insight]` - a realization that changes understanding
- `[idea]`, `[proposal]`, `[poc]` - speculative or draft content; never mark speculation as `[fact]` or `[decision]`

Relations connect engrams to each other: `- depends_on [[Other Engram]]`, or a quoted multi-word type like `- "relates to" [[Other Engram]]`.

## Temporal fields

`valid_from`/`valid_to` are optional. Absence is the normal case and it means unbounded: no `valid_from` means the engram has always been valid, no `valid_to` means it is valid forever. Set them only when validity is genuinely bounded - a policy that changes on a known date, a temporary workaround with a known expiry or a superseded fact whose end date is known (the supersede recipe below closes that window). Never write a sentinel far-future date to mean "forever"; just omit the field.

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

Bounds are not top-level write arguments and go inside metadata; the write enforces the format - a value that is not a plain ISO date (YYYY-MM-DD) fails with an error naming the field, and a sentinel far-future valid_to or an explicit null is dropped since absence already means valid forever, so a successful bounded write needs no read-back confirmation.

## Superseding instead of contradicting

Do not let an engram hold both current and outdated guidance at once. When new knowledge replaces old knowledge:

1. Write the replacement as its own new engram with `status: stable` - do not rewrite the old engram's factual content in place, which skips step 2 and leaves the outdated fact searchable as current.
2. Edit the old engram: `edit_engram` with `operation: "set_frontmatter"`, `key: "status"` and `value: "superseded"` (or `"deprecated"`). It assigns the field by name, so it lands whether the old engram spells its current state `stable` or the legacy `current`, and there is nothing to match and nothing to guess.
3. Add a `- superseded_by [[New Engram]]` relation on the old engram (and, optionally, `- supersedes [[Old Engram]]` on the new one).

A `set_frontmatter` status flip either lands or errors, so there is no read-back to do; what still needs checking is step 3, since a `[[Target]]` whose text does not match a real title stays unresolved even though the edit succeeded.

When the date the old fact stopped holding is known, close its validity window right after the status flip: a second `set_frontmatter` call with `key: "valid_to"` and the date as `value`. Use the real-world transition date, not the date you happened to notice - and when that date is unknown, leave the window open and let `status` alone mark the retirement. A closed window is what makes the past addressable later: a search with `metadata_filters` on `valid_from`/`valid_to` can then answer "what applied last June" with the engram that was true then, the way a person recalls how things worked at a past job without mistaking it for the present.

Retiring knowledge is also the moment to learn from it. The superseded engram keeps the full experience; any insight that outlives it - why the old approach failed, what to watch for next time - is unbounded knowledge that belongs as a `[lesson]` or `[pattern]` bullet on the new engram or its own engram, linked back to the retired one. The experience stays time-scoped; what it taught carries forward without bounds.

Not every retirement has a replacement. When nothing takes the old fact's place - a practice abandoned, a tool dropped, a caveat that stopped applying - skip step 1 and retire in place: flip `status` to a retirement value, close `valid_to` when the end date is known and carry any surviving insight forward as a `[lesson]`. Staleness you notice while reading earns the same treatment, proposed to the user first rather than done silently. The status words each mean one thing: `deprecated` says do not do this again, `superseded` says a newer engram replaced this one, `archived` says retired but kept for the record and `legacy` says still deployed and true of old installations but not to be built on.

## Working a maintenance queue

When the user asks what the archive itself needs - what has gone stale, what is half-finished, what looks duplicated - `evolve_engrams` sweeps a domain or every domain read-only and returns a ranked queue where each finding carries its evidence and its next action: work the items marked `mechanical` directly and summarize once, propose each `judgment` item and wait for a yes one at a time, and never read a finding as proof that two engrams disagree, since the sweep detects by dates, links and graph shape rather than by meaning. A tag drift finding is the one fix you never apply yourself: hand the user the `crystalline tags merge` command it names and let them run it.

## Confirm before destroying

Always confirm with the user before calling `delete_engram` or `move_engram` - describe what will be removed or relocated and wait for a yes. Prefer setting `status` to `deprecated` or `superseded` over deleting when the history is still worth keeping; `move_engram` on a cross-domain move rewrites inbound bare links to the domain-prefixed form automatically unless `update_links` is set to `false`. A destination inside the same domain is an ordinary move too: it is how a topic cluster gets its subfolder after the fact.
