import { describe, expect, it } from "vitest";

import { AMBIENCES, CROSSFADE_S, dronePatch } from "./ambience";
import type { Ambience } from "./cues";
import type { Patch } from "./patch";

/** The lowest pitch any tonal voice of a drone starts at. */
function fundamental(patch: Patch): number {
  const tonal = patch.voices.filter((v) => v.wave !== "noise");
  return Math.min(...tonal.map((v) => v.pitch[0]!.value));
}

describe("dronePatch", () => {
  // Mutation caught: a drone that ends (no loop), one with no voice, or
  // one that starts at full level (a click on entry instead of a fade in).
  it("loops a voiced drone for every ambience, fading in", () => {
    expect(AMBIENCES).toHaveLength(8);
    for (const kind of AMBIENCES) {
      const patch = dronePatch(kind, 7);
      expect(patch.loop, kind).toBe(true);
      expect(patch.voices.length, kind).toBeGreaterThan(0);
      for (const v of patch.voices) {
        expect(v.env.a, kind).toBeGreaterThanOrEqual(CROSSFADE_S);
        expect(v.gain, kind).toBeGreaterThan(0);
        expect(v.gain, kind).toBeLessThanOrEqual(1);
      }
    }
  });

  // Mutation caught: two kinds sharing one patch, except the dark room,
  // which plays the derelict drone by rule (M4 C22).
  it("gives every kind its own drone, the dark room the derelict's", () => {
    const voices = (kind: Ambience) =>
      JSON.stringify(dronePatch(kind, 7).voices);
    for (const a of AMBIENCES) {
      for (const b of AMBIENCES) {
        if (a >= b) continue;
        if (
          (a === "dark" && b === "derelict") ||
          (a === "derelict" && b === "dark")
        )
          continue;
        expect(voices(a), `${a} vs ${b}`).not.toBe(voices(b));
      }
    }
    expect(voices("dark")).toBe(voices("derelict"));
  });

  // Mutation caught: the hum of every room at one pitch, the hangar's
  // rumble above a room's, or the console room's below.
  it("sets the hangar lowest and the console room highest (M4 C22)", () => {
    const hz = new Map(
      AMBIENCES.map((k) => [k, fundamental(dronePatch(k, 7))]),
    );
    const hangar = hz.get("hangar")!;
    const console_ = hz.get("console")!;
    for (const [kind, f] of hz) {
      if (kind !== "hangar") expect(f, kind).toBeGreaterThan(hangar);
      if (kind !== "console") expect(f, kind).toBeLessThan(console_);
    }
    expect(hz.get("clean")).toBeCloseTo(55, 0);
  });

  // Mutation caught: the derelict drone held steady (no drift).
  it("lets the derelict drone waver in pitch", () => {
    const drifting = dronePatch("derelict", 7).voices.filter(
      (v) => v.drift !== undefined && v.drift.cents > 0,
    );
    expect(drifting.length).toBeGreaterThan(0);
    for (const v of drifting) expect(v.drift!.rate).toBeCloseTo(0.07, 9);
    expect(
      dronePatch("clean", 7).voices.some((v) => v.drift !== undefined),
    ).toBe(false);
  });

  // Mutation caught: an unseeded detune (two calls differing).
  it("is the same drone for the same kind and seed", () => {
    for (const kind of AMBIENCES) {
      expect(dronePatch(kind, 3)).toEqual(dronePatch(kind, 3));
    }
  });
});
