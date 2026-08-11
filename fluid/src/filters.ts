/**
 * The frontmatter filters: how a URL is read as one, and the values the two
 * free-form ones are offered as.
 *
 * `type` and `status` are free form by design and no endpoint enumerates the
 * values a domain actually uses, so these are the vocabulary the product
 * recommends rather than a claim about any instance. Every screen that filters
 * offers them as suggestions in a datalist for that reason: anything can be
 * typed, and nothing here says a value not on the list is wrong.
 *
 * One list, shared, so the domain screen and the search screen never drift
 * into recommending different words for the same field.
 */

import type { EngramFilters } from "./api/engrams";
import { RETIRED_STATUSES } from "./lifecycle";

/**
 * The frontmatter filters a domain URL carries, as one value.
 *
 * Shared rather than read twice, because two readers disagreeing about
 * whether a filter is on is exactly the bug it was written for: the screen
 * decides which of its two views to draw from this, and the frame decides
 * whether any folder may call itself the current page. One reading, one
 * answer, and `hasFilters` over it means the same thing in both places.
 *
 * `path` is deliberately empty. It is a scope rather than a filter, and the
 * filtered view is the whole domain, every folder included.
 */
export function frontmatterFilters(params: URLSearchParams): EngramFilters {
  return {
    type: params.get("type"),
    status: params.get("status"),
    tags: (params.get("tags") ?? "").split(",").filter((tag) => tag !== ""),
    path: "",
  };
}

/** The recommended `type` values. */
export const SUGGESTED_TYPES = [
  "engram",
  "guide",
  "decision",
  "architecture",
  "runbook",
  "reference",
];

/** The recommended `status` values, retired ones included. */
export const SUGGESTED_STATUSES = [
  "stable",
  "current",
  "draft",
  "proposed",
  "idea",
  "poc",
  ...RETIRED_STATUSES,
];
