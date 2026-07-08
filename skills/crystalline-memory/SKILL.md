---
name: crystalline-memory
description: Use when the crystalline MCP tools are available and the task involves recalling, storing or sharing knowledge - searching domains, capturing engrams or sharing with a team.
---

# Crystalline Memory

Crystalline gives you durable memory across sessions: what you have been taught lives in Domains as Engrams you search, read and, where allowed, capture into.

## Start from the routing block

At connection you are handed a routing block as your instructions: one line per registered domain plus the tool names it routes through. Treat each line as a targeting aid, not a full catalog - an unrelated-looking domain may still hold the answer. Re-fetch the same index mid-session with `list_domains` and `include_routing: true`.

## Recall before you answer

Never answer from pre-trained knowledge alone when a domain could cover it; search first. This holds even for a question that looks self-contained or seems to need clarification - a question that feels ambiguous is a signal to search first, not to ask the user before searching. Only ask a clarifying question once a search comes back genuinely empty or irrelevant. One domain obviously owns the task: a scoped search (`domains: ["that-domain"]`); broad or cross-cutting: a sweep with `domains` omitted. Decide that scope by matching the question's subject against the routing block's domain lines before you search, not after seeing results - when a domain's description names the topic (say a payments domain whose line mentions refunds and retries), scope to it on the first call rather than sweeping unscoped out of caution. For a "what is true now" question, filter on `status: current`. Follow a strong hit with `build_context` to pull in what surrounds it - it returns titles and relations only, so reads follow it. If a first phrasing turns up nothing, reformulate once before concluding the knowledge was not captured.

A search hit's snippet is a short window around the match, not the engram: `read_engram` an engram before citing or summarizing its content, and pass it a single `identifier` string like `"domain/permalink"`, not separate `permalink` and `domain` fields. For an overview question ("what is X about?") read every strong hit before drafting - what you read earlier for a different task is not coverage for this one - and when something relevant stays unread, say so instead of presenting partial coverage as complete.

That scope is a starting bet, not a lock-in. If every hit in the scoped domain is tangential - a different sub-topic, or just its MANIFEST - even after you reformulate once, the answer may live in a domain the question's surface noun does not name: an escalation contact, an on-call owner or a policy question about a technical topic often sits in a people or ops domain rather than the technical one. Retry as an unscoped sweep before concluding the knowledge is missing or asking the user.

For a question bound to a specific past date or window ("what applied on 2025-08-01", "the cost in June 2025"), do not trust text-score ranking alone - pass `metadata_filters` on `valid_from` and `valid_to` so the engram whose window actually contains that date wins over a superseded or not-yet-current one with a similar-looking snippet.

Once a hit answers the question, stop there. Reading the domain MANIFEST (`read_engram` on it, or `browse_domain`) is an extra step for when the routing line and the hits together still leave the right engram unclear, not a routine follow-up to every search - and since the MANIFEST is itself indexed, it often surfaces among the hits already.

## Capture what you learn

Treat capture as a normal byproduct of the task, not an end-of-session chore. If the user did not ask for it, propose first: name the insight and the domain, and wait for a yes. Searching before you write is mandatory, never optional - run `search_engrams` for the topic before any write tool, even when the request is a short clarification that feels self-contained. If a hit already owns the topic, even a broader engram whose scope covers this narrower case, edit that engram rather than creating a new one beside it. Every write names its domain explicitly - there is no default, so knowledge never lands in the wrong place by accident. A body is markdown bullets: `- [category] content #tag` for an observation, `- rel_type [[Target]]` for a relation. Leave `valid_from` and `valid_to` unset unless a fact is genuinely time-bounded - absence already means always valid, never a sentinel date. Mark old knowledge `superseded` or `deprecated` rather than letting it stand alongside the new as current. Confirm with the user before deleting or moving an engram.

For a domain in your routing block, crystalline is where its knowledge lives: route every "remember this" or "update what we know" request through the crystalline tools, never a generic file `Write` or a local memory folder. Those are separate stores, and a fact written there is invisible to every future search here. If a write tool you reached for is missing or errors, that is the cue to fall back to `search_engrams`, not to try another store.

Content the user flags as transient or disregardable - a scratch note, an "ignore this", a one-off local workaround - is not a capture candidate: do not propose storing it, just answer any real question alongside it and move on. When one message bundles a transient aside with a genuinely capture-worthy fact, drop the aside and still run the full search-then-append-or-create pass on the fact alone.

When new information replaces a fact already held in a current engram, do not `find_replace` that engram's text to the new value - that erases the history the supersede model exists to keep. Leave the old engram's content as it stands, flip its `status` to `superseded` and add a `- superseded_by [[New Engram]]` relation, then write the new fact as its own engram with `status: current` and a `- supersedes [[Old Engram]]` relation pointing back. This is only for a genuine replacement or contradiction; an exception or extra detail about a fact that still holds is an append to its engram, not a supersede.

When you are asked to link a new fact to an existing engram, make the link resolve both ways: add the relation on the new engram pointing at the existing one and a matching relation on the existing engram pointing back, so each is discoverable from the other.

A write only counts as captured once it passes the verify layer: `tags` present, at least three non-blank content lines, valid dates, no duplicate permalink and every `[[Target]]` relation resolving to a real engram. So a single-bullet body fails the line count - give even a small fact a lead-in sentence plus two bullets, say the fact itself and its scope or source. A `[[Target]]` resolves by exact title, capitalization included, so a relation whose text does not match its target's real title dangles even though the write itself succeeds. And a validity bound only sticks when `valid_from` and `valid_to` are set as keys inside the `metadata` object - a bound stated only in the body prose, or a `metadata` passed as a JSON string instead of an object, never becomes a real frontmatter field, so read the engram back and confirm the window landed before you report it as bounded.

## Read-only deployments

Some deployments are read-only and say so up front: the instructions state the knowledge is curated externally, and the write tools are simply absent from the tool list. Search and read there; do not propose a capture.

## Sharing with a team

When origin tools like `share_changes` and `origin_status` are visible, some domains are shared with a team. Call `origin_status` before deep work, and `update_domain` first if behind. Share a coherent unit of knowledge with `share_changes` and always relay the returned review URL - a person merges the proposal, never the agent. Settle a disagreement with `resolve_conflict`: `mine`, `theirs` or a supplied `merged` version.

Team collaboration is set up through `configure` alone, no terminal needed: setting `github.enabled` reveals the tools above, and `connect: "github"` returns a code and a URL for a browser already signed in to GitHub. Git and clones are never part of this flow.
