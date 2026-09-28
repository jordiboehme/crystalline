/**
 * The ambience: the looping drone of every kind of room (M4 C22).
 *
 * A drone is a looping patch the director keeps on the ambience bus and
 * cross-fades over `CROSSFADE_S` when the player enters a room of another
 * kind. Its body is two pulse voices a few cents apart (they beat slowly
 * against each other) through a lowpass at four to six times the
 * fundamental, their widths swung slowly by a width LFO (`pwm`, since a
 * looping patch holds its steps). Noise voices carry the airlock's hiss
 * and the derelict room's crackle.
 *
 * - clean: 55 Hz.
 * - construction: 52 Hz with a slow 0.2 Hz tremolo, machinery at work.
 * - dim: 46 Hz with a darker filter.
 * - derelict: 41 Hz wavering (a 0.07 Hz pitch drift) with a faint crackle.
 * - airlock: the clean hum under a soft air hiss that breathes.
 * - hangar: a lower, wider rumble at 33 Hz, its two voices further apart.
 * - console: a warm machinery hum at 73 Hz swelling every 6 s.
 * - dark: the derelict drone (a room gone dark, C19).
 *
 * The room's seed picks the detune (two to six cents) and the width LFO's
 * rate, so rooms of a kind differ a little and a room always sounds the
 * same. Every voice fades in over `CROSSFADE_S` (its attack), the other
 * half of the director's cross-fade.
 */

import { seedFor } from "../core/seed";
import type { Ambience } from "./cues";
import type { Patch, Voice } from "./patch";

/** How long a drone takes to fade in and the one it replaces to fade out. */
export const CROSSFADE_S = 0.6;

/** Every ambience, in the order the sound board lists them. */
export const AMBIENCES = [
  "clean",
  "construction",
  "dim",
  "derelict",
  "airlock",
  "hangar",
  "console",
  "dark",
] as const satisfies readonly Ambience[];

/** How one kind's hum is made. */
interface Hum {
  /** The fundamental, Hz. */
  hz: number;
  /** The lowpass as a multiple of the fundamental (4 to 6). */
  filter: number;
  /** The level of each of the two pulse voices. */
  gain: number;
  /** Extra detune between the two voices, cents, on top of the seed's. */
  spread?: number;
  tremolo?: Voice["tremolo"];
  drift?: Voice["drift"];
  /** The noise voices over the hum. */
  noise?: readonly Voice[];
}

/** A looping voice's envelope: a fade in over the cross-fade, then held. */
const FADE_IN = { a: CROSSFADE_S, d: 0.2, s: 0.9, r: 0.3 } as const;

/** A looping noise voice: `gain`, a filter, an optional tremolo. */
function noise(
  gain: number,
  filter: NonNullable<Voice["filter"]>,
  tremolo?: Voice["tremolo"],
): Voice {
  return {
    wave: "noise",
    pitch: [],
    env: { ...FADE_IN },
    length: 1,
    at: 0,
    gain,
    filter,
    ...(tremolo === undefined ? {} : { tremolo }),
  };
}

/** The derelict drone, which the dark room plays too. */
const DERELICT: Hum = {
  hz: 41,
  filter: 4,
  gain: 0.32,
  drift: { rate: 0.07, cents: 35 },
  noise: [
    // The crackle: a thin band of noise sputtering on a fast deep tremolo.
    noise(
      0.05,
      { type: "bandpass", cutoff: [{ at: 0, value: 2600 }], q: 1.5 },
      { rate: 7.3, depth: 1 },
    ),
  ],
};

/** Every kind's hum (M4 C22). */
const HUMS = {
  clean: { hz: 55, filter: 5, gain: 0.32 },
  construction: {
    hz: 52,
    filter: 5,
    gain: 0.32,
    tremolo: { rate: 0.2, depth: 0.5 },
  },
  dim: { hz: 46, filter: 4, gain: 0.32 },
  derelict: DERELICT,
  airlock: {
    hz: 55,
    filter: 5,
    gain: 0.28,
    noise: [
      noise(
        0.14,
        { type: "bandpass", cutoff: [{ at: 0, value: 1400 }], q: 0.7 },
        { rate: 0.15, depth: 0.4 },
      ),
    ],
  },
  hangar: { hz: 33, filter: 6, gain: 0.36, spread: 8 },
  console: {
    hz: 73,
    filter: 4,
    gain: 0.3,
    tremolo: { rate: 1 / 6, depth: 0.6 },
  },
  dark: DERELICT,
} as const satisfies Record<Ambience, Hum>;

/** The drone of `ambience` for the room of `seed`: a looping patch. */
export function dronePatch(ambience: Ambience, seed: number): Patch {
  const hum: Hum = HUMS[ambience];
  const pick = seedFor("drone", seed);
  const cents = 2 + (pick % 5) + (hum.spread ?? 0);
  const rate = 0.08 + ((pick >>> 4) % 5) * 0.02;
  const pulse = (detune: number, width: number): Voice => {
    const hz = hum.hz * 2 ** (detune / 1200);
    return {
      wave: "pulse",
      pitch: [{ at: 0, value: hz }],
      width: [{ at: 0, value: width }],
      pwm: { rate, depth: 0.2 },
      filter: {
        type: "lowpass",
        cutoff: [{ at: 0, value: hum.hz * hum.filter }],
        q: 1,
      },
      env: { ...FADE_IN },
      length: 1,
      at: 0,
      gain: hum.gain,
      ...(hum.tremolo === undefined ? {} : { tremolo: hum.tremolo }),
      ...(hum.drift === undefined ? {} : { drift: hum.drift }),
    };
  };
  return {
    name: `drone ${ambience}`,
    loop: true,
    voices: [pulse(0, 0.45), pulse(cents, 0.35), ...(hum.noise ?? [])],
  };
}
