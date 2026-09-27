/**
 * The level cheat's word: `idclev`, typed anywhere on the game route with
 * no prompt, opens the level select (the spec's IDCLEV section).
 *
 * The session feeds it every code the player types (`Input.typed`), in
 * order, with the tick it was read on, and it answers true for the V that
 * completes the word and false for everything else. No letter of the word
 * is swallowed: E is no longer the use key (Space is), so the word typed in
 * front of a terminal never opens the CRT reader, and its D strafes for a
 * moment, as in the classic game.
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

/**
 * Reads the word out of the keys typed, one at a time.
 *
 * - `feed` takes one code and the tick it was read on (never decreasing)
 *   and answers whether it completed the word.
 * - `reset` forgets a half-typed word: the session calls it whenever an
 *   overlay opens or closes (C4).
 */
export interface CheatReader {
  feed(code: string, tick: number): boolean;
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
      if (typed < IDCLEV.length) return false;
      typed = 0;
      return true;
    },
    reset() {
      typed = 0;
      last = -Infinity;
    },
  };
}
