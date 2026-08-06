/**
 * Which statuses mean "retired", and what the app does about them.
 *
 * `status` is free form, so this is a recognition rule rather than an
 * enumeration: a status outside this set is shown as written and treated as
 * live. A retired engram is faded wherever it is listed and never hidden - it
 * is part of what the domain holds, and a list that quietly dropped it would
 * misrepresent the domain to the person reading it.
 */

/** The statuses that mean an engram has been retired. */
export const RETIRED_STATUSES = [
  "deprecated",
  "superseded",
  "archived",
  "legacy",
] as const;

/** Whether this status is one of them. */
export function isRetired(status: string | null | undefined): boolean {
  return (
    status !== null &&
    status !== undefined &&
    (RETIRED_STATUSES as readonly string[]).includes(status.toLowerCase())
  );
}

/** The class that fades a retired row. One definition, so every list fades alike. */
export const RETIRED_CLASS = "opacity-60";
