import { describe, expect, it } from "vitest";

import {
  MIN_EXP,
  arpeggio,
  midiHz,
  patchLength,
  valueAt,
  type Patch,
  type Voice,
} from "./patch";

/** A voice with every field set, for tests that change one or two. */
function voice(over: Partial<Voice> = {}): Voice {
  return {
    wave: "saw",
    pitch: [{ at: 0, value: 220 }],
    env: { a: 0.01, d: 0.1, s: 0.5, r: 0.2 },
    length: 0.5,
    at: 0,
    gain: 1,
    ...over,
  };
}

describe("midiHz", () => {
  // Mutation caught: the reference note or the octave off (69 -> 440 fails
  // for any other anchor, 57 -> 220 for a wrong divisor).
  it("tunes note 69 to 440 Hz and an octave down to 220 Hz", () => {
    expect(midiHz(69)).toBe(440);
    expect(midiHz(57)).toBeCloseTo(220, 10);
    expect(midiHz(81)).toBeCloseTo(880, 10);
  });
});

describe("arpeggio", () => {
  // Mutation caught: an off-by-one step (a sixth step at 0.1, or four
  // steps), the notes not cycled, or the steps ramped instead of held.
  it("cycles the notes every 20 ms over the length", () => {
    const steps = arpeggio([60, 64, 67], 0.1);
    expect(steps).toHaveLength(5);
    const hz = [60, 64, 67, 60, 64].map(midiHz);
    steps.forEach((step, i) => {
      expect(step.at).toBeCloseTo(i * 0.02, 10);
      expect(step.value).toBe(hz[i]);
      expect(step.ramp).toBeUndefined();
    });
  });

  // Mutation caught: the step length ignored.
  it("takes another step length", () => {
    const steps = arpeggio([69, 81], 0.1, 0.05);
    expect(steps.map((s) => s.at)).toEqual([0, 0.05]);
    expect(steps.map((s) => s.value)).toEqual([440, 880]);
  });
});

describe("patchLength", () => {
  // Mutation caught: the release left out, the start offset left out, or
  // the first voice read instead of the longest.
  it("ends when the last voice's release ends", () => {
    const patch: Patch = {
      name: "two",
      voices: [
        voice({ at: 0, length: 1, env: { a: 0, d: 0, s: 1, r: 0.3 } }),
        voice({ at: 0.5, length: 0.6, env: { a: 0, d: 0, s: 1, r: 0.4 } }),
      ],
    };
    expect(patchLength(patch)).toBeCloseTo(1.5, 10);
  });

  // Mutation caught: a reduce seeded with -Infinity (an empty patch
  // would never end).
  it("is zero for no voices", () => {
    expect(patchLength({ name: "none", voices: [] })).toBe(0);
  });
});

describe("valueAt", () => {
  // Mutation caught: a ramp read as a hold (the midpoint would read 100),
  // or the value after the last step not held.
  it("interpolates a linear ramp and holds after the last step", () => {
    const steps = [
      { at: 0, value: 100 },
      { at: 1, value: 200, ramp: "linear" as const },
    ];
    expect(valueAt(steps, 0)).toBe(100);
    expect(valueAt(steps, 0.5)).toBeCloseTo(150, 10);
    expect(valueAt(steps, 1)).toBe(200);
    expect(valueAt(steps, 3)).toBe(200);
  });

  // Mutation caught: a step without a ramp interpolated.
  it("holds a plain step until the next one", () => {
    const steps = [
      { at: 0, value: 100 },
      { at: 1, value: 200 },
    ];
    expect(valueAt(steps, 0.99)).toBe(100);
    expect(valueAt(steps, 1)).toBe(200);
  });

  // Mutation caught: an exponential ramp read as a linear one (the
  // midpoint of 100 to 400 is 200 on an exponential ramp, 250 on a line).
  it("interpolates an exponential ramp geometrically", () => {
    const steps = [
      { at: 0, value: 100 },
      { at: 2, value: 400, ramp: "exp" as const },
    ];
    expect(valueAt(steps, 1)).toBeCloseTo(200, 10);
  });

  // Mutation caught: an exponential ramp from 0 interpolated linearly
  // (WebAudio holds the earlier value when it is not positive), or a ramp
  // to 0 read as 0 (the synth floors the target at MIN_EXP).
  it("reads exponential ramps as WebAudio plays them", () => {
    const fromZero = [
      { at: 0, value: 0 },
      { at: 1, value: 100, ramp: "exp" as const },
    ];
    expect(valueAt(fromZero, 0.5)).toBe(0);
    expect(valueAt(fromZero, 1)).toBe(100);
    const toZero = [
      { at: 0, value: 100 },
      { at: 1, value: 0, ramp: "exp" as const },
    ];
    expect(valueAt(toZero, 1)).toBe(MIN_EXP);
    expect(valueAt(toZero, 0.5)).toBeCloseTo(Math.sqrt(100 * MIN_EXP), 12);
  });

  // Mutation caught: the time before the first step reading zero.
  it("reads the first value before the first step", () => {
    expect(valueAt([{ at: 0.5, value: 7 }], 0)).toBe(7);
  });
});
