/**
 * The blink banks (H11): how a hero's lights pulse, flash, twinkle, swap
 * and chase without anything moving.
 *
 * A hero instance carries a bank in its instance slot (attribute 7's `y`,
 * `bankSlot` of its kind's bank), and each of its lights a group inside
 * that bank in its flag (`FLAG.blink + group`, `blinkFlag` in
 * `geometry.ts`). The shader reads the light's gain from one uniform array,
 * `uBlink[slot * BLINK_GROUPS + group]`, and multiplies only its signal
 * formula by it (and by `uGain`, as every exit is). There are seven banks
 * of eight groups:
 *
 * - `steady` (slot 0): every group at 1. The static room, the movers and
 *   the props all read slot 0, so they are unchanged.
 * - `breathe`: DOOM's glow special at level 255, one cycle recorded once
 *   from `createLights` (`GLOW_CYCLE`); each group reads it at its own
 *   seeded phase, since the glow itself draws no random numbers and would
 *   otherwise run every group in step.
 * - `status` and `twinkle`: DOOM's strobe and flicker specials, one light
 *   zone of level 255 per group, each group with its own seed, so the
 *   groups of a bank are out of step.
 * - `swap`: groups 0 to 3 lit while 4 to 7 are low, the other way round
 *   every `SWAP_TICS`: a face or a screen that flips between two pictures.
 * - `chase`: one group lit at a time, the next every `CHASE_TICS`: a light
 *   running round a ring.
 * - `soft`: the breathe bank's glow, group for group at the same phase,
 *   lifted to run from `SOFT_FLOOR` to 1 instead of from the glow's own low
 *   level: a light that breathes softly and never dims far, for a glow set
 *   just under the bloom threshold that must still read lit at its lowest
 *   (the console room's glowing roundels, 2.6e C26).
 *
 * A low group reads `BLINK_LOW`, a lit one 1; the DOOM banks move between
 * their special's low level and 255, divided by 255, and never below
 * `BLINK_LOW`.
 *
 * Like the light specials (`lights.ts`), all of this runs on the CPU once
 * per 35 Hz tick, counted in ticks and seeded by names, so the
 * hand-cranked clock of the session test drives it exactly. It reaches the
 * shader as one uniform array of `BLINK_CHANNELS` floats. It is not per
 * room: a hero blinks the same way wherever it stands.
 */

import { seedFor } from "../core/seed";
import type { LightSpecial, LightZone } from "../world/types";
import { createLights } from "./lights";

/** The blink banks, in slot order; slot 0 is the steady bank (H11). */
export const BLINK_BANKS = [
  "steady",
  "breathe",
  "status",
  "twinkle",
  "swap",
  "chase",
  "soft",
] as const;
/** One bank's name. */
export type BlinkBank = (typeof BLINK_BANKS)[number];
/** Groups per bank: a part's flag is `FLAG.blink + group`. */
export const BLINK_GROUPS = 8;
/** Gains the shader reads, bank by bank: `uBlink[slot * BLINK_GROUPS + group]`. */
export const BLINK_CHANNELS = BLINK_BANKS.length * BLINK_GROUPS;
/** A dark group of the swap and chase banks: dim glass, not black. */
export const BLINK_LOW = 0.15;
/** Ticks the swap bank holds each half: 3 s at 35 Hz. */
export const SWAP_TICS = 105;
/** Ticks the chase bank holds each group lit. */
export const CHASE_TICS = 5;
/**
 * The soft bank's lowest gain: its glow runs from here to 1 (2.6e C26), so
 * a light on it never falls more than a tenth under its peak.
 */
export const SOFT_FLOOR = 0.9;

/**
 * The DOOM special behind each bank run as light zones: the strobe and the
 * flicker draw from each zone's seed, so their groups fall out of step by
 * themselves. The breathe bank's glow draws nothing and is phased by hand
 * (`GLOW_CYCLE`).
 */
const SPECIAL: Partial<Record<BlinkBank, LightSpecial>> = {
  status: "strobe",
  twinkle: "flicker",
};

/** The longest glow cycle `GLOW_CYCLE` will record before it gives up, in ticks. */
const GLOW_CYCLE_MAX = 1000;

/**
 * One whole cycle of DOOM's glow at level 255, tick by tick from its start
 * (255, heading down, through its low level and back up): recorded once
 * from `createLights` with a single glow zone, so the breathe bank moves
 * exactly as a glowing light zone does. It ends on the tick before the
 * glow is back at 255.
 */
const GLOW_CYCLE: readonly number[] = (() => {
  const glow = createLights([
    { x0: 0, y0: 0, x1: 1, y1: 1, level: 255, special: "glow", seed: 0 },
  ]);
  const cycle = [glow.levels[0] ?? 255];
  for (let t = 0; t < GLOW_CYCLE_MAX; t++) {
    glow.tick();
    const level = glow.levels[0] ?? 255;
    if (level === 255) return cycle;
    cycle.push(level);
  }
  throw new Error("blink: the glow never came back to its level");
})();

/** The glow's lowest level in `GLOW_CYCLE`, as a gain. */
const GLOW_LOW = Math.min(...GLOW_CYCLE) / 255;

/** A bank's slot: its index in `BLINK_BANKS`. */
export function bankSlot(bank: BlinkBank): number {
  return BLINK_BANKS.indexOf(bank);
}

/** Every channel's gain, advanced one 35 Hz tick at a time. */
export interface BlinkState {
  readonly gains: Float32Array;
  tick(): void;
}

/**
 * The blink state: the breathe bank read from `GLOW_CYCLE`, each group at
 * the phase `seedFor("blink", "breathe", group)` gives it; the status and
 * twinkle banks as light zones of level 255, one per group, each with its
 * own seed (so the groups of a bank are out of step), run by
 * `createLights`; the swap and chase banks counted from the ticks.
 */
export function createBlink(): BlinkState {
  const gains = new Float32Array(BLINK_CHANNELS).fill(1);
  // Each bank's first channel, looked up once rather than on every tick.
  const base = (bank: BlinkBank) => bankSlot(bank) * BLINK_GROUPS;
  const breathe = base("breathe");
  const swap = base("swap");
  const chase = base("chase");
  const soft = base("soft");
  const phases = Array.from(
    { length: BLINK_GROUPS },
    (_, g) => seedFor("blink", "breathe", g) % GLOW_CYCLE.length,
  );
  const doom = BLINK_BANKS.flatMap((bank) => {
    const special = SPECIAL[bank];
    return special === undefined ? [] : [{ at: base(bank), bank, special }];
  });
  const zones: LightZone[] = doom.flatMap(({ bank, special }) =>
    Array.from({ length: BLINK_GROUPS }, (_, g) => ({
      x0: 0,
      y0: 0,
      x1: 1,
      y1: 1,
      level: 255,
      special,
      seed: seedFor("blink", bank, g),
    })),
  );
  const lights = createLights(zones);
  let ticks = 0;
  const fill = () => {
    // DOOM's flicker and strobe low level is round(0.15 * 255) = 38, and
    // 38 / 255 is 0.149, a hair under BLINK_LOW: every DOOM gain is
    // clamped, so no group of any bank is ever darker than a swap or chase
    // group. The glow's low level (0.4) is above it anyway.
    doom.forEach(({ at }, k) => {
      for (let g = 0; g < BLINK_GROUPS; g++)
        gains[at + g] = Math.max(
          BLINK_LOW,
          (lights.levels[k * BLINK_GROUPS + g] ?? 255) / 255,
        );
    });
    const first = Math.floor(ticks / SWAP_TICS) % 2 === 0;
    const lit = Math.floor(ticks / CHASE_TICS) % BLINK_GROUPS;
    for (let g = 0; g < BLINK_GROUPS; g++) {
      const glow =
        GLOW_CYCLE[(ticks + (phases[g] ?? 0)) % GLOW_CYCLE.length] ?? 255;
      gains[breathe + g] = Math.max(BLINK_LOW, glow / 255);
      gains[soft + g] =
        SOFT_FLOOR +
        ((1 - SOFT_FLOOR) * (glow / 255 - GLOW_LOW)) / (1 - GLOW_LOW);
      gains[swap + g] = g < BLINK_GROUPS / 2 === first ? 1 : BLINK_LOW;
      gains[chase + g] = g === lit ? 1 : BLINK_LOW;
    }
  };
  fill();
  return {
    gains,
    tick() {
      ticks++;
      lights.tick();
      fill();
    },
  };
}
