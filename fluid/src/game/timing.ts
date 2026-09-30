/**
 * Timings shared by the session and the sound, in a module that imports
 * nothing, so a pure module such as `audio/effects.ts` can read one
 * without pulling the session (and its renderer, GL and world) into its
 * graph. `session.ts` re-exports them for its own readers.
 */

/**
 * How long a lift ride lasts at the least, in milliseconds (M3 C27): a
 * load that settles sooner is held until then, so the connector naming
 * the stop stays up for the ride. The ride's hum rises over this long
 * (`audio/effects.ts`, F16).
 */
export const LIFT_RIDE_MS = 1200;
