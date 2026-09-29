/**
 * The signature sounds (M4 C24) and the answering console (C25).
 *
 * - **The five tones**: the first-contact motif (`FIVE_TONES`), a call on
 *   the organ voice and, after a pause, a slower response an octave lower
 *   on a pulse through a lowpass. The level select's jump plays it, and so
 *   does a terminal that answers.
 * - **The wheeze**: the police box's take-off (the cut into the console
 *   room) and its landing (the step out of the arrival box on a bridge).
 *   Our own, from the synth alone: three breaths of a detuned pair of saws
 *   and a grinding noise through a resonant bandpass that sweeps up and
 *   back down within each breath. The take-off fades out over the three,
 *   the landing fades in and ends with a thump.
 * - **The answering console**: about one terminal in `ANSWER_EVERY`,
 *   picked from its fixture's seed at runtime (nothing in `RoomSpec`
 *   changes), answers with the tones once per visit after the player has
 *   faced it for `ANSWER_WAIT_MS` (the session times it).
 *
 * Each function answers a `Patch` (`audio/patch.ts`) the director plays on
 * the `signature` bus; nothing here touches WebAudio. As everywhere in a
 * patch, every step's `at` runs from the patch start, so a later note's or
 * breath's steps are written from its own start.
 */

import { seedFor } from "../core/seed";
import type { Fixture } from "../world/types";
import { midiHz, type Patch, type Step, type Voice } from "./patch";

/** The first-contact motif as MIDI notes: up a tone, down a third, down an octave, up a fifth. */
export const FIVE_TONES = [67, 69, 65, 53, 60] as const;

/** One terminal in this many answers (M4 C25). */
export const ANSWER_EVERY = 48;

/** How long the player faces an answering console before it answers (C25). */
export const ANSWER_WAIT_MS = 1500;

/** The call's notes: their gate, the gap after each, the last one's gate. */
const CALL = { note: 0.42, gap: 0.04, last: 0.9 } as const;

/**
 * The response's notes, and the pause before it. The plan had 0.55 s
 * notes, a last of 1.2 s and a 0.6 s pause, which with the call's 2.74 s
 * comes to 6.8 s against its own cap of 6 s; the notes here stay longer
 * than the call's, so the response still reads as slower, and the whole
 * ends just under 6 s.
 */
const RESPONSE = { pause: 0.35, note: 0.46, gap: 0.04, last: 0.8 } as const;

/** Where a run of five notes starts each, and how long each gate is. */
function notes(
  from: number,
  spec: { note: number; gap: number; last: number },
): { at: number; length: number }[] {
  return FIVE_TONES.map((_, i) => ({
    at: from + i * (spec.note + spec.gap),
    length: i === FIVE_TONES.length - 1 ? spec.last : spec.note,
  }));
}

/** The call on the organ voice and the slower response an octave lower (M4 C24). */
export function tonesPatch(): Patch {
  const call = notes(0, CALL);
  const callEnd = (call.at(-1)?.at ?? 0) + CALL.last;
  const response = notes(callEnd + RESPONSE.pause, RESPONSE);
  const voices: Voice[] = [
    ...call.map(({ at, length }, i): Voice => ({
      wave: "organ",
      pitch: [{ at, value: midiHz(FIVE_TONES[i] as number) }],
      env: { a: 0.06, d: 0.12, s: 0.8, r: 0.1 },
      length,
      at,
      gain: 0.4,
    })),
    ...response.map(({ at, length }, i): Voice => ({
      wave: "pulse",
      pitch: [{ at, value: midiHz((FIVE_TONES[i] as number) - 12) }],
      width: [{ at, value: 0.35 }],
      filter: { type: "lowpass", cutoff: [{ at, value: 900 }], q: 1 },
      env: { a: 0.06, d: 0.15, s: 0.8, r: 0.1 },
      length,
      at,
      gain: 0.36,
    })),
  ];
  return { name: "five tones", voices };
}

/**
 * The loudest breath's voice gain. The band at Q 8 passes little of a saw
 * or of the noise, so it is high: at 0.9 a render measured the loudest
 * breath at a peak of 0.31, half a door's.
 */
const BREATH_GAIN = 1.7;

/** A breath's length, and the breaths' count. */
const BREATH_S = 1.1;
const BREATHS = 3;

/** A move from `lo` up to `hi` at the breath's middle and back, from `at`. */
function upAndBack(at: number, lo: number, hi: number): Step[] {
  return [
    { at, value: lo },
    { at: at + BREATH_S / 2, value: hi, ramp: "exp" },
    { at: at + BREATH_S, value: lo, ramp: "exp" },
  ];
}

/**
 * One breath of the wheeze from `at`, scaled by `level`: two saws at 148.5
 * and 151.5 Hz, their pitch wobbling up 2 % and back, and noise with a
 * grinding 7 Hz tremolo, all through a bandpass at Q 8 sweeping 300 to
 * 1600 Hz and back. A slow attack and release make it swell and ebb.
 */
function breath(at: number, level: number): Voice[] {
  const env = { a: 0.4, d: 0.1, s: 0.9, r: 0.25 };
  const length = BREATH_S - env.r;
  const band = () => ({
    type: "bandpass" as const,
    cutoff: upAndBack(at, 300, 1600),
    q: 8,
  });
  const saw = (hz: number): Voice => ({
    wave: "saw",
    pitch: upAndBack(at, hz, hz * 1.02),
    filter: band(),
    env,
    length,
    at,
    gain: BREATH_GAIN * level,
  });
  return [
    saw(148.5),
    saw(151.5),
    {
      wave: "noise",
      pitch: [],
      filter: band(),
      env,
      length,
      at,
      gain: BREATH_GAIN * level,
      tremolo: { rate: 7, depth: 0.45 },
    },
  ];
}

/** The breaths at `levels`, one after the other. */
const breaths = (levels: readonly number[]): Voice[] =>
  levels.slice(0, BREATHS).flatMap((level, i) => breath(i * BREATH_S, level));

/** The take-off wheeze (entering the police box). */
export function takeoffPatch(): Patch {
  return { name: "wheeze takeoff", voices: breaths([1, 0.7, 0.45]) };
}

/**
 * The landing wheeze with its final thump (stepping out of the arrival
 * box): the breaths fading in, then 0.2 s after the last a 20 ms noise
 * transient over a sine falling 90 to 40 Hz in 0.3 s.
 */
export function landingPatch(): Patch {
  const at = BREATHS * BREATH_S + 0.2;
  return {
    name: "wheeze landing",
    voices: [
      ...breaths([0.45, 0.7, 1]),
      {
        wave: "noise",
        pitch: [],
        filter: { type: "lowpass", cutoff: [{ at, value: 1200 }], q: 1 },
        env: { a: 0.001, d: 0.015, s: 0.3, r: 0.03 },
        length: 0.02,
        at,
        gain: 0.35,
      },
      {
        wave: "sine",
        pitch: [
          { at, value: 90 },
          { at: at + 0.3, value: 40, ramp: "exp" },
        ],
        env: { a: 0.003, d: 0.15, s: 0.5, r: 0.15 },
        length: 0.3,
        at,
        gain: 0.5,
      },
    ],
  };
}

/** Whether a terminal is an answering console (M4 C25). */
export function answers(fixture: Fixture): boolean {
  return (
    fixture.kind === "terminal" &&
    seedFor(fixture.seed, "answer") % ANSWER_EVERY === 0
  );
}
