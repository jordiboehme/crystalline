/**
 * The values the two free-form frontmatter filters are offered as.
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

import { RETIRED_STATUSES } from "./lifecycle";

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
