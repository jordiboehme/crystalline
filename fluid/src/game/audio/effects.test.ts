import { describe, expect, it } from "vitest";

import { LIFT_RIDE_MS } from "../timing";
import type { DoorSound } from "./cues";
import {
  doorPatch,
  faultPatch,
  hatchPatch,
  portalPatch,
  ridePatch,
  stepPatch,
  terminalPatch,
} from "./effects";
import { patchLength, valueAt, type Patch, type Voice } from "./patch";

const DOORS: readonly DoorSound[] = [
  "sliding",
  "bulkhead",
  "blast",
  "exit",
  "box",
];
const WAYS = ["door", "hatch", "portal"] as const;

/** Every one-shot and the ride's two phases, as the director plays them. */
function everyEffect(): Patch[] {
  return [
    stepPatch(0, false, 0),
    stepPatch(1, false, 1),
    stepPatch(7, true, 0),
    stepPatch(8, true, 1),
    ...DOORS.flatMap((s) => [doorPatch(s, true), doorPatch(s, false)]),
    hatchPatch(),
    portalPatch(),
    terminalPatch(),
    ...WAYS.flatMap((w) => [1, 2, 3].map((run) => faultPatch(w, run))),
    ridePatch("depart"),
    ridePatch("arrive"),
  ];
}

/** The thump of a footstep: its only sine voice. */
function thump(patch: Patch): Voice {
  const v = patch.voices.find((voice) => voice.wave === "sine");
  if (v === undefined) throw new Error(`${patch.name} has no thump`);
  return v;
}

describe("effects", () => {
  // Mutation caught: a 60 s patch, a patch with no voice, a gain above 1
  // (clipping on the effects bus) or at 0 (a voice nobody hears).
  it("keeps every patch short, voiced and in gain", () => {
    for (const patch of everyEffect()) {
      expect(patchLength(patch), patch.name).toBeLessThanOrEqual(6);
      expect(patch.voices.length, patch.name).toBeGreaterThan(0);
      for (const v of patch.voices) {
        expect(v.gain, patch.name).toBeGreaterThan(0);
        expect(v.gain, patch.name).toBeLessThanOrEqual(1);
      }
    }
  });

  // Mutation caught: one patch for both directions of a door.
  it("sounds a door's opening and its closing differently", () => {
    for (const sound of DOORS) {
      // The voices, not the names, which always differ.
      expect(doorPatch(sound, true).voices, sound).not.toEqual(
        doorPatch(sound, false).voices,
      );
    }
  });

  // Mutation caught: a constant step, a random pitch (the same arguments
  // giving two patches), the foot ignored, or running as loud as walking.
  it("varies a step's pitch by its number, a semitone lower on foot 1", () => {
    const pitches = new Set<number>();
    for (let n = 0; n < 50; n++) {
      const a = stepPatch(n, false, 0);
      expect(stepPatch(n, false, 0)).toEqual(a);
      const hz = thump(a).pitch[0]!.value;
      pitches.add(hz);
      const lower = thump(stepPatch(n, false, 1)).pitch[0]!.value;
      expect(lower / hz).toBeCloseTo(2 ** (-1 / 12), 9);
    }
    expect(pitches.size).toBeGreaterThan(1);
    const walk = Math.max(...stepPatch(3, false, 0).voices.map((v) => v.gain));
    const run = Math.max(...stepPatch(3, true, 0).voices.map((v) => v.gain));
    expect(run).toBeGreaterThan(walk);
  });

  // Mutation caught: the fault's run ignored (every run the same sound,
  // F15).
  it("sounds a fault's runs differently", () => {
    for (const way of WAYS) {
      const voices = (run: number) => faultPatch(way, run).voices;
      expect(voices(1), way).not.toEqual(voices(2));
      expect(voices(2), way).not.toEqual(voices(3));
      expect(faultPatch(way, 4), way).toEqual(faultPatch(way, 4));
    }
  });

  // Mutation caught: a ride hum that ends before the ride (no loop), or
  // one that reaches its top before the ride has lasted `LIFT_RIDE_MS`.
  it("holds the ride's hum, risen over the ride, until it is stopped (F16)", () => {
    const depart = ridePatch("depart");
    expect(depart.loop).toBe(true);
    const ride = LIFT_RIDE_MS / 1000;
    for (const v of depart.voices) {
      const top = valueAt(v.pitch, ride);
      expect(valueAt(v.pitch, ride / 2)).toBeLessThan(top);
      expect(valueAt(v.pitch, ride * 10)).toBeCloseTo(top, 9);
      expect(valueAt(v.pitch, 0)).toBeLessThan(top);
    }
    expect(ridePatch("arrive").loop).not.toBe(true);
  });
});
