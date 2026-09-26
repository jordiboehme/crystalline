/**
 * The level cheat's word: `idclev`, typed anywhere on the game route with
 * no prompt, opens the level select (the spec's IDCLEV section).
 *
 * The session feeds it every code the player types (`Input.typed`), in
 * order, with the tick it was read on, and it answers what that key means
 * for the word:
 *
 * - `swallow` for the E right after `I D C L`, so the word typed in front
 *   of a terminal never opens the CRT reader (every other E is a use);
 * - `match` for the V that completes it;
 * - `none` for everything else.
 *
 * Letters are `KeyboardEvent.code`s, the physical positions the game reads
 * all its keys by (C1). A letter more than `CHEAT_GAP_TICKS` after the one
 * before it starts the word over (C2), so an I pressed long ago to invert
 * the look never becomes the start of a word, whose match would take that
 * toggle back. No letter of the word but the first is an I, so on a wrong
 * key the word restarts at one letter when that key is an I and at none
 * otherwise; nothing longer can be a prefix again.
 */

/** The word, as the codes of its letters. */
export const IDCLEV = ["KeyI", "KeyD", "KeyC", "KeyL", "KeyE", "KeyV"] as const;

/**
 * The longest pause between two letters of the word, in 35 Hz ticks: one
 * second (C2).
 */
export const CHEAT_GAP_TICKS = 35;

/** The index of the word's E, the letter that is swallowed. */
const E_AT = IDCLEV.indexOf("KeyE");

/** What one typed key means for the word. See the module doc. */
export type CheatStep = "none" | "swallow" | "match";

/**
 * Reads the word out of the keys typed, one at a time.
 *
 * - `feed` takes one code and the tick it was read on (never decreasing)
 *   and answers its `CheatStep`.
 * - `reset` forgets a half-typed word: the session calls it whenever an
 *   overlay opens or closes (C4).
 */
export interface CheatReader {
  feed(code: string, tick: number): CheatStep;
  reset(): void;
}

/** A reader with no letter of the word typed yet. */
export function createCheatReader(): CheatReader {
  let typed = 0;
  let last = -Infinity;
  return {
    feed(code, tick) {
      if (tick - last > CHEAT_GAP_TICKS) typed = 0;
      last = tick;
      if (code === IDCLEV[typed]) {
        typed++;
      } else {
        typed = code === IDCLEV[0] ? 1 : 0;
      }
      if (typed === IDCLEV.length) {
        typed = 0;
        return "match";
      }
      return typed === E_AT + 1 ? "swallow" : "none";
    },
    reset() {
      typed = 0;
      last = -Infinity;
    },
  };
}
