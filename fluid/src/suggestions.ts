/**
 * What the recommended `type` and `status` words mean, in one line each.
 *
 * The lists themselves live in `filters.ts`, because filtering and authoring
 * must not recommend different vocabularies for the same field; this module
 * only says what each word is for. The glosses are the reason the authoring
 * fields can stop expecting anybody to have memorized the set: a status
 * picker that shows ten words and explains none of them is a spelling aid,
 * not guidance.
 *
 * A module of its own rather than a few more lines in `filters.ts` because
 * only the two authoring surfaces need the prose, and both of them are lazy
 * chunks: the filter bar on the eagerly loaded screens keeps paying for the
 * words alone.
 *
 * Nothing here is enforced anywhere. A value with no gloss is simply a value
 * this app has no advice about, which is the normal case for a domain's own
 * words.
 */

import type { NamedCount } from "./api/vocabulary";
import type { Suggestion } from "./components/SuggestInput";
import { SUGGESTED_STATUSES, SUGGESTED_TYPES } from "./filters";

/** What each recommended `type` is for. */
const TYPE_GLOSSES: Record<string, string> = {
  engram: "a unit of knowledge, and the default",
  guide: "how to do something, start to finish",
  decision: "a choice that was made, and why",
  architecture: "how a system is put together",
  runbook: "the steps to take when something happens",
  reference: "facts to look up rather than read through",
};

/** What each recommended `status` says about an engram. */
const STATUS_GLOSSES: Record<string, string> = {
  stable: "holds now, and the default",
  implemented: "built and in place",
  draft: "still being written",
  proposed: "put forward, not agreed yet",
  idea: "a thought worth keeping",
  poc: "a proof of concept: shown to work once, in a prototype",
  deprecated: "still here, no longer advised",
  superseded: "replaced by a newer engram",
  archived: "kept for the record, out of use",
  legacy: "from an older way of working",
  current: "the older word for stable; a filter on either matches both",
};

/** Pair a list of values with whatever this app can say about each. */
function glossed(
  names: readonly string[],
  glosses: Record<string, string>,
): Suggestion[] {
  return names.map((name) => {
    const gloss = glosses[name];
    return gloss === undefined ? { name } : { name, gloss };
  });
}

/** The recommended `type` values, each with its one line. */
export const TYPE_SUGGESTIONS: Suggestion[] = glossed(
  SUGGESTED_TYPES,
  TYPE_GLOSSES,
);

/** The recommended `status` values, each with its one line. */
export const STATUS_SUGGESTIONS: Suggestion[] = glossed(
  SUGGESTED_STATUSES,
  STATUS_GLOSSES,
);

/**
 * Recommended suggestions enriched with what the domain actually uses:
 * matching names gain their live count, house-only values are appended
 * (count desc, then name) so a domain's own vocabulary is one keystroke
 * away without losing the glossed recommendations.
 *
 * The recommendations keep their order and their place at the top, because
 * they are the advice; the house words come after because they are already
 * known to whoever writes them and need no explaining. Neither list is a
 * closed set - an appended word has no gloss because this app has nothing to
 * say about it, not because it is somehow lesser.
 */
export function withHouseCounts(
  base: readonly Suggestion[],
  house: readonly NamedCount[],
): Suggestion[] {
  const counts = new Map(house.map((each) => [each.name, each.count]));
  const merged = base.map((suggestion) => {
    const count = counts.get(suggestion.name);
    return count === undefined ? suggestion : { ...suggestion, count };
  });
  const recommended = new Set(base.map((suggestion) => suggestion.name));
  const extra = house
    .filter((each) => !recommended.has(each.name))
    .sort((a, b) => b.count - a.count || a.name.localeCompare(b.name))
    .map(({ name, count }) => ({ name, count }));
  return [...merged, ...extra];
}
