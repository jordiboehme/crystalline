/**
 * The effects: the patch of every one-shot the station plays (M4 C23).
 *
 * Each function answers a `Patch` (`audio/patch.ts`), data the director
 * (`audio/director.ts`) plays on the effects bus and the dev sound board
 * lists by name. Nothing here touches WebAudio. The recipes follow the
 * plan's; where they go further, the doc of the function says so:
 *
 * - **Steps**: a short noise tick through a lowpass that falls as it
 *   plays (the heel), over a low sine thump whose pitch is picked from the
 *   step's number (`seedFor("step", n)`), foot 1 a semitone below foot 0,
 *   louder and brighter when running.
 * - **Doors**: a sliding hiss (a bandpass sweep of noise, falling to open,
 *   rising to close), a bulkhead's square clunk with the hiss (the clunk
 *   first on opening, last on closing: the latch), a blast door's heavy
 *   low saw with a long hiss (the same order), a police box's creak, and a
 *   room's exit as a sliding door. A closing sliding door lands with a
 *   soft thud on the frame.
 * - **The hatch**: a pop and a puff of air, the `travel` cue's.
 * - **The portal**: a whoosh over a 50 Hz arpeggio.
 * - **The terminal**: a short pulse blip.
 * - **Faults**: a door's grind and slam, a hatch's rattle, a portal's
 *   collapsing zap; each run a semitone apart from the one before (F15).
 * - **The lift ride**: `depart` a hum rising over the ride and then held
 *   (a looping patch, stopped by the director on `arrive`, F16), `arrive`
 *   a two-note chime.
 *
 * Every patch's name is fixed per recipe (not per step or run), since the
 * synth fills a noise buffer per name and keeps it.
 */

import { seedFor } from "../core/seed";
import { LIFT_RIDE_MS } from "../session";
import type { DoorSound } from "./cues";
import { arpeggio, midiHz, type Patch, type Step, type Voice } from "./patch";

/** The ride's length in seconds: the depart hum's rise. */
const RIDE_S = LIFT_RIDE_MS / 1000;

/** A semitone as a pitch ratio. */
const SEMITONE = 2 ** (1 / 12);

/** A voice from its wave, gain, gate and envelope, the rest as given. */
function voice(
  wave: Voice["wave"],
  gain: number,
  length: number,
  env: Voice["env"],
  over: Partial<Voice> = {},
): Voice {
  return { wave, pitch: [], env, length, at: 0, gain, ...over };
}

/** A two-step move from `from` to `to`, ramped exponentially over [t0, t1]. */
function sweep(from: number, to: number, t0: number, t1: number): Step[] {
  return [
    { at: t0, value: from },
    { at: t1, value: to, ramp: "exp" },
  ];
}

/**
 * One footstep (M4 C23, F15): a 25 ms noise tick through a lowpass at 900
 * Hz walking or 1400 Hz running, falling to a third of that over the tick
 * (a heel rather than a hiss), and a 60 ms sine thump at 70 to 90 Hz
 * picked by `seedFor("step", n) % 21`, gliding down a little. Foot 1 is a
 * semitone below foot 0. Gain 0.25 walking, 0.4 running.
 */
export function stepPatch(n: number, run: boolean, foot: 0 | 1): Patch {
  const gain = run ? 0.4 : 0.25;
  const cutoff = run ? 1400 : 900;
  const hz = (70 + (seedFor("step", n) % 21)) / SEMITONE ** foot;
  return {
    name: run ? "step run" : "step walk",
    voices: [
      voice(
        "noise",
        gain,
        0.025,
        { a: 0.001, d: 0.015, s: 0.4, r: 0.02 },
        {
          filter: {
            type: "lowpass",
            cutoff: sweep(cutoff, cutoff / 3, 0, 0.025),
            q: 1,
          },
        },
      ),
      voice(
        "sine",
        gain,
        0.06,
        { a: 0.002, d: 0.04, s: 0.3, r: 0.04 },
        { pitch: sweep(hz, hz * 0.75, 0, 0.08) },
      ),
    ],
  };
}

/**
 * The sliding hiss from `at`: noise through a bandpass swept 2500 to 900
 * Hz over 0.45 s (opening) or 900 to 2500 Hz (closing), gain `gain`: 0.5
 * for a sliding door, where the plan had 0.35, since a render measured
 * the hiss through the band far quieter than the other doors.
 */
function hiss(open: boolean, at: number, gain: number, span = 0.45): Voice {
  const [from, to] = open ? [2500, 900] : [900, 2500];
  return voice(
    "noise",
    gain,
    span,
    { a: 0.04, d: 0.1, s: 0.75, r: 0.12 },
    {
      at,
      filter: {
        type: "bandpass",
        cutoff: sweep(from, to, at, at + span),
        q: 2,
      },
    },
  );
}

/** A soft low thud at `at`: a door meeting its frame. */
function thud(at: number, gain: number, hz = 65): Voice {
  return voice(
    "sine",
    gain,
    0.07,
    { a: 0.002, d: 0.05, s: 0.3, r: 0.08 },
    { at, pitch: sweep(hz, hz * 0.6, at, at + 0.12) },
  );
}

/** A bulkhead's clunk at `at`: a 90 ms square at 110 Hz through a lowpass. */
function clunk(at: number, gain: number, hz = 110): Voice {
  return voice(
    "pulse",
    gain,
    0.09,
    { a: 0.002, d: 0.05, s: 0.35, r: 0.06 },
    {
      at,
      pitch: [{ at, value: hz }],
      width: [{ at, value: 0.5 }],
      filter: { type: "lowpass", cutoff: sweep(900, 250, at, at + 0.09), q: 2 },
    },
  );
}

/**
 * A door-like thing turning to open (`open`) or to shut (M4 C23): see the
 * module doc. The exit sounds as a sliding door.
 */
export function doorPatch(sound: DoorSound, open: boolean): Patch {
  const name = `door ${sound} ${open ? "open" : "close"}`;
  switch (sound) {
    case "sliding":
    case "exit":
      return {
        name,
        voices: open
          ? [hiss(true, 0, 0.5)]
          : [hiss(false, 0, 0.5), thud(0.42, 0.35)],
      };
    case "bulkhead":
      return {
        name,
        voices: open
          ? [clunk(0, 0.45), hiss(true, 0.08, 0.4)]
          : [hiss(false, 0, 0.4), clunk(0.42, 0.45)],
      };
    case "blast": {
      // A 0.25 s low saw at 45 Hz, its lowpass falling 400 to 120 Hz.
      const slam = (at: number): Voice =>
        voice(
          "saw",
          0.6,
          0.25,
          { a: 0.004, d: 0.15, s: 0.45, r: 0.2 },
          {
            at,
            pitch: [{ at, value: 45 }],
            filter: {
              type: "lowpass",
              cutoff: sweep(400, 120, at, at + 0.25),
              q: 1.5,
            },
          },
        );
      return {
        name,
        voices: open
          ? [slam(0), hiss(true, 0.15, 0.4, 1.2)]
          : [hiss(false, 0, 0.4, 1.2), slam(1.1)],
      };
    }
    case "box": {
      // A creak: a saw through a narrow bandpass at about its third
      // harmonic (higher, the band left almost nothing of it), a stutter
      // of tremolo on it for the grain of old hinges; closing ends in a
      // latch's click.
      const [from, to] = open ? [180, 140] : [140, 180];
      const creak = voice(
        "saw",
        0.6,
        0.5,
        { a: 0.03, d: 0.1, s: 0.8, r: 0.1 },
        {
          pitch: sweep(from, to, 0, 0.5),
          filter: { type: "bandpass", cutoff: [{ at: 0, value: 560 }], q: 6 },
          tremolo: { rate: 11, depth: 0.5 },
        },
      );
      return {
        name,
        voices: open ? [creak] : [creak, pop(0.5, 0.25, 520, 260)],
      };
    }
  }
}

/** A short triangle pop from `from` to `to` Hz at `at`. */
function pop(at: number, gain: number, from = 220, to = 90): Voice {
  return voice(
    "triangle",
    gain,
    0.04,
    { a: 0.001, d: 0.03, s: 0.3, r: 0.03 },
    { at, pitch: sweep(from, to, at, at + 0.04) },
  );
}

/**
 * The hatch's pop as it is taken (the `travel` cue `hatch`): a 40 ms
 * triangle 220 to 90 Hz, and a quiet puff of air after it.
 */
export function hatchPatch(): Patch {
  return {
    name: "hatch",
    voices: [
      pop(0, 0.5),
      voice(
        "noise",
        0.15,
        0.08,
        { a: 0.01, d: 0.05, s: 0.4, r: 0.1 },
        {
          at: 0.02,
          filter: {
            type: "lowpass",
            cutoff: sweep(2400, 600, 0.02, 0.2),
            q: 1,
          },
        },
      ),
    ],
  };
}

/**
 * The portal's whoosh on travel: noise through a bandpass swept 300 to
 * 3000 Hz and back over 0.6 s, a pulse under it cycling midi 60, 67 and
 * 72 at 50 Hz, its width opening as it goes.
 */
export function portalPatch(): Patch {
  return {
    name: "portal",
    voices: [
      voice(
        "noise",
        0.4,
        0.6,
        { a: 0.08, d: 0.2, s: 0.7, r: 0.2 },
        {
          filter: {
            type: "bandpass",
            cutoff: [
              { at: 0, value: 300 },
              { at: 0.3, value: 3000, ramp: "exp" },
              { at: 0.6, value: 300, ramp: "exp" },
            ],
            q: 3,
          },
        },
      ),
      voice(
        "pulse",
        0.18,
        0.6,
        { a: 0.01, d: 0.1, s: 0.7, r: 0.15 },
        {
          pitch: arpeggio([60, 67, 72], 0.6),
          width: [
            { at: 0, value: 0.2 },
            { at: 0.6, value: 0.5, ramp: "linear" },
          ],
        },
      ),
    ],
  };
}

/** The terminal's blip: a 70 ms pulse at midi 88, width 0.25. */
export function terminalPatch(): Patch {
  return {
    name: "terminal",
    voices: [
      voice(
        "pulse",
        0.3,
        0.07,
        { a: 0.002, d: 0.02, s: 0.6, r: 0.08 },
        {
          pitch: [{ at: 0, value: midiHz(88) }],
          width: [{ at: 0, value: 0.25 }],
        },
      ),
    ],
  };
}

/**
 * A broken way's run (M4 C23, F15), one sound per run, each run a
 * semitone off the one before (runs cycle through 0, +1 and -1): a door's
 * 0.4 s grind (a 60 Hz saw through a bandpass with an 18 Hz tremolo) and
 * then a slam (the bulkhead's clunk, lower), a hatch's three 30 ms rattles
 * 80 ms apart, a portal's zap (a pulse falling 2000 to 80 Hz over 0.35 s).
 */
export function faultPatch(
  way: "door" | "hatch" | "portal",
  run: number,
): Patch {
  const shift = SEMITONE ** ([0, 1, -1][((run % 3) + 3) % 3] as number);
  const name = `fault ${way}`;
  switch (way) {
    case "door":
      return {
        name,
        voices: [
          voice(
            "saw",
            0.45,
            0.4,
            { a: 0.01, d: 0.1, s: 0.8, r: 0.05 },
            {
              pitch: [
                { at: 0, value: 60 * shift },
                { at: 0.4, value: 52 * shift, ramp: "exp" },
              ],
              filter: {
                type: "bandpass",
                cutoff: [{ at: 0, value: 420 * shift }],
                q: 3,
              },
              tremolo: { rate: 18, depth: 0.8 },
            },
          ),
          clunk(0.42, 0.5, 90 * shift),
        ],
      };
    case "hatch":
      return {
        name,
        voices: [0, 0.08, 0.16].map((at, i) =>
          voice(
            "pulse",
            0.6 - i * 0.1,
            0.03,
            { a: 0.001, d: 0.02, s: 0.3, r: 0.02 },
            {
              at,
              pitch: sweep(240 * shift, 160 * shift, at, at + 0.03),
              width: [{ at, value: 0.15 }],
              filter: { type: "bandpass", cutoff: [{ at, value: 1200 }], q: 2 },
            },
          ),
        ),
      };
    case "portal":
      return {
        name,
        voices: [
          voice(
            "pulse",
            0.35,
            0.35,
            { a: 0.002, d: 0.1, s: 0.7, r: 0.08 },
            {
              pitch: sweep(2000 * shift, 80 * shift, 0, 0.35),
              width: [
                { at: 0, value: 0.1 },
                { at: 0.35, value: 0.5, ramp: "linear" },
              ],
            },
          ),
        ],
      };
  }
}

/**
 * The lift ride (F16). `depart`: two saws a few cents apart humming from
 * 55 Hz up to 82 Hz over the ride's length (`LIFT_RIDE_MS`), a lowpass
 * opening with them, then held: the patch loops, since the ride's real
 * length is unknown when it departs, and the director stops it on
 * `arrive`. `arrive`: the chime, sine midi 84 then 79.
 */
export function ridePatch(phase: "depart" | "arrive"): Patch {
  if (phase === "arrive") {
    const bell = (at: number, note: number, length: number): Voice =>
      voice(
        "sine",
        0.3,
        length,
        { a: 0.004, d: 0.15, s: 0.5, r: 0.7 },
        { at, pitch: [{ at, value: midiHz(note) }] },
      );
    return {
      name: "ride arrive",
      voices: [bell(0, 84, 0.2), bell(0.28, 79, 0.3)],
    };
  }
  const hum = (cents: number, gain: number): Voice =>
    voice(
      "saw",
      gain,
      RIDE_S,
      { a: 0.3, d: 0.2, s: 0.85, r: 0.2 },
      {
        pitch: sweep(
          55 * 2 ** (cents / 1200),
          82 * 2 ** (cents / 1200),
          0,
          RIDE_S,
        ),
        filter: { type: "lowpass", cutoff: sweep(160, 700, 0, RIDE_S), q: 3 },
      },
    );
  return {
    name: "ride depart",
    loop: true,
    voices: [hum(0, 0.3), hum(7, 0.22)],
  };
}
