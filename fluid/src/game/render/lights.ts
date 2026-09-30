/**
 * The classic light specials, evaluated at the classic 35 Hz tick and with
 * the classic numbers.
 *
 * Every light zone has a level on a 0 to 255 scale and a special that
 * moves it: a glow that breathes up and down, a fire-like flicker, a strobe,
 * and a failing tube that blinks between on and off at random. The constants
 * are the classic ones, counted in tics, which is why the loop runs at
 * 35 Hz. The classic engine takes a special's low level from the darkest
 * neighbouring sector; a room here has no neighbours, so the low level is a
 * fixed fraction of the zone's own.
 *
 * All of this runs on the CPU once per tick and reaches the shader as one
 * float per zone. Randomness comes from each zone's own seed, so a failing
 * light fails the same way on every visit.
 */

import { createRng, type Rng } from "../core/seed";
import type { LightZone } from "../world/types";

/** How many levels a glow moves per tic. */
export const GLOWSPEED = 8;
/** How many tics a strobe stays bright. */
export const STROBEBRIGHT = 5;
/** How many tics a fast strobe stays dark. */
export const FASTDARK = 15;
/** How many tics a slow strobe stays dark (unused by the specials here, kept for parity with the classic table). */
export const SLOWDARK = 35;
/** How many tics between one flicker step and the next. */
export const FLICKER_TICS = 4;
/**
 * The classic bounds on a failing light's random hold time, in tics:
 * `FLASH_MIN` for a dark spell, `FLASH_MAX` for a lit one. The classic
 * engine masks its random byte with these; here they bound a uniform
 * draw instead, so the hold time is `1` to `FLASH_MIN + 1` tics dark and
 * `1` to `FLASH_MAX + 1` tics lit.
 */
export const FLASH_MIN = 7;
/**
 * The upper bound, in tics, on how long a failing light stays lit before it
 * drops dark again: the hold time is drawn from `1` to `FLASH_MAX + 1`. See
 * `FLASH_MIN` above for where both bounds come from.
 */
export const FLASH_MAX = 64;

/** The zones' current levels, advanced one tick at a time. */
export interface LightState {
  readonly levels: Float32Array;
  tick(): void;
}

/** The low end of a zone's special. */
export function minLevelOf(zone: LightZone): number {
  return Math.round(zone.level * (zone.special === "glow" ? 0.4 : 0.15));
}

interface Runner {
  rng: Rng;
  count: number;
  direction: number;
  level: number;
}

/** Light state over `zones`, in their order. */
export function createLights(zones: readonly LightZone[]): LightState {
  const levels = new Float32Array(zones.length);
  const runners: Runner[] = zones.map((z, i) => {
    levels[i] = z.level;
    const rng = createRng(z.seed);
    return { rng, count: rng.int(1, 8), direction: -1, level: z.level };
  });

  const tick = () => {
    zones.forEach((z, i) => {
      const r = runners[i];
      if (r === undefined) return;
      const max = z.level;
      const min = minLevelOf(z);
      switch (z.special) {
        case "steady":
          r.level = max;
          break;
        case "glow":
          r.level += r.direction * GLOWSPEED;
          if (r.level <= min) {
            r.level = min;
            r.direction = 1;
          } else if (r.level >= max) {
            r.level = max;
            r.direction = -1;
          }
          break;
        case "flicker":
          if (--r.count <= 0) {
            const amount = r.rng.int(0, 3) * 16;
            r.level = Math.max(max - amount, min);
            r.count = FLICKER_TICS;
          }
          break;
        case "strobe":
          if (--r.count <= 0) {
            if (r.level === min) {
              r.level = max;
              r.count = STROBEBRIGHT;
            } else {
              r.level = min;
              r.count = FASTDARK;
            }
          }
          break;
        case "failing":
          if (--r.count <= 0) {
            if (r.level === max) {
              r.level = min;
              r.count = r.rng.int(0, FLASH_MIN) + 1;
            } else {
              r.level = max;
              r.count = r.rng.int(0, FLASH_MAX) + 1;
            }
          }
          break;
      }
      levels[i] = r.level;
    });
  };

  return { levels, tick };
}
