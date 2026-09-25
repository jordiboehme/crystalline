/**
 * The blink banks (H11): how a hero's lights pulse, flash, twinkle, swap
 * and chase without anything moving.
 *
 * A hero instance carries a bank in its instance slot (attribute 7's `y`,
 * `bankSlot` of its kind's bank), and each of its lights a group inside
 * that bank in its flag (`FLAG.blink + group`, `blinkFlag` in
 * `geometry.ts`). The shader reads the light's gain from one uniform array,
 * `uBlink[slot * BLINK_GROUPS + group]`, and multiplies only its signal
 * formula by it (and by `uGain`, as every exit is). There are six banks of
 * eight groups:
 *
 * - `steady` (slot 0): every group at 1. The static room, the movers and
 *   the props all read slot 0, so they are unchanged.
 * - `breathe`, `status` and `twinkle`: DOOM's glow, strobe and flicker
 *   specials, one light zone of level 255 per group, each group with its
 *   own seed so the groups of a bank are out of step.
 * - `swap`: groups 0 to 3 lit while 4 to 7 are low, the other way round
 *   every `SWAP_TICS`: a face or a screen that flips between two pictures.
 * - `chase`: one group lit at a time, the next every `CHASE_TICS`: a light
 *   running round a ring.
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

/** The DOOM special behind each bank that has one. */
const SPECIAL: Partial<Record<BlinkBank, LightSpecial>> = {
  breathe: "glow",
  status: "strobe",
  twinkle: "flicker",
};

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
 * The blink state: the DOOM banks as light zones of level 255, one per
 * group, each with its own seed (so the groups of a bank are out of step),
 * run by `createLights`; the swap and chase banks counted from the ticks.
 */
export function createBlink(): BlinkState {
  const gains = new Float32Array(BLINK_CHANNELS).fill(1);
  const doom = BLINK_BANKS.flatMap((bank) => {
    const special = SPECIAL[bank];
    return special === undefined ? [] : [{ bank, special }];
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
    doom.forEach(({ bank }, k) => {
      for (let g = 0; g < BLINK_GROUPS; g++)
        // DOOM's flicker and strobe low level is round(0.15 * 255) = 38,
        // and 38 / 255 is 0.149, a hair under BLINK_LOW: clamp, so no
        // group of any bank is ever darker than a swap or chase group.
        gains[bankSlot(bank) * BLINK_GROUPS + g] = Math.max(
          BLINK_LOW,
          (lights.levels[k * BLINK_GROUPS + g] ?? 255) / 255,
        );
    });
    const first = Math.floor(ticks / SWAP_TICS) % 2 === 0;
    const lit = Math.floor(ticks / CHASE_TICS) % BLINK_GROUPS;
    for (let g = 0; g < BLINK_GROUPS; g++) {
      gains[bankSlot("swap") * BLINK_GROUPS + g] =
        g < BLINK_GROUPS / 2 === first ? 1 : BLINK_LOW;
      gains[bankSlot("chase") * BLINK_GROUPS + g] = g === lit ? 1 : BLINK_LOW;
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
