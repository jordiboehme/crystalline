/**
 * The three statuses the guided retire dialog offers.
 *
 * The retire endpoint's own contract (`POST /domains/{d}/retire`), not
 * `lifecycle.ts`'s recognition list: that file says which of any free-form
 * status word means "retired" once written, and this says which three words
 * the endpoint itself accepts as a status to retire an engram into.
 */
export const RETIREMENT_STATUSES = [
  "deprecated",
  "superseded",
  "archived",
] as const;
