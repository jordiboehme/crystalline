import { describe, expect, it } from "vitest";

import {
  CANNED_BRIDGE,
  CANNED_HUB,
  CANNED_WORKSHOP,
  galleryRoom,
  heroHallRoom,
} from "../world/canned";
import { generateRoom } from "../world/generate";
import type { Fixture, RoomSpec } from "../world/types";
import { midiHz, patchLength, type Patch, type Voice } from "./patch";
import {
  ANSWER_EVERY,
  FIVE_TONES,
  answers,
  landingPatch,
  takeoffPatch,
  tonesPatch,
} from "./signature";

/** The first pitch of a voice. */
const firstHz = (v: Voice | undefined) => v?.pitch[0]?.value;

/** The breaths of a wheeze: its voices grouped by start, in order. */
function breaths(patch: Patch): Voice[][] {
  const by = new Map<number, Voice[]>();
  for (const v of patch.voices) {
    if (v.wave !== "saw") continue;
    by.set(v.at, [...(by.get(v.at) ?? []), v]);
  }
  return [...by.entries()].sort(([a], [b]) => a - b).map(([, vs]) => vs);
}

/** The loudest voice gain of each breath. */
const breathGains = (patch: Patch) =>
  breaths(patch).map((vs) => Math.max(...vs.map((v) => v.gain)));

/** A terminal fixture with `seed`, the rest of it plain. */
const terminal = (seed: number): Fixture => ({
  kind: "terminal",
  slot: { x: 1, y: 0, side: "n" },
  heading: "Scope",
  lines: [],
  section: 0,
  seed,
});

describe("the five tones (M4 C24)", () => {
  // Mutation caught: the motif reordered, the response at the call's
  // octave.
  it("plays the call on the organ, then the response an octave lower", () => {
    const patch = tonesPatch();
    const call = patch.voices.slice(0, 5);
    const response = patch.voices.slice(5, 10);
    expect(call.map(firstHz)).toEqual(FIVE_TONES.map(midiHz));
    expect(call.every((v) => v.wave === "organ")).toBe(true);
    expect(response.map(firstHz)).toEqual(
      FIVE_TONES.map((n) => midiHz(n - 12)),
    );
    expect(response.every((v) => v.wave === "pulse")).toBe(true);
    // The response starts after the call has ended, and is slower.
    const callEnd = Math.max(...call.map((v) => v.at + v.length));
    expect(Math.min(...response.map((v) => v.at))).toBeGreaterThan(callEnd);
    expect(response[0]!.length).toBeGreaterThan(call[0]!.length);
    // Each note is in time order.
    for (const part of [call, response]) {
      for (let i = 1; i < part.length; i++) {
        expect(part[i]!.at).toBeGreaterThan(part[i - 1]!.at);
      }
    }
  });

  it("lasts at most 6 s", () => {
    expect(patchLength(tonesPatch())).toBeLessThanOrEqual(6);
    expect(patchLength(tonesPatch())).toBeGreaterThan(4);
  });
});

describe("the wheeze (M4 C24)", () => {
  // Mutation caught: the thump on the take-off, the fade the wrong way.
  it("fades out over three breaths on the take-off, with no thump", () => {
    const patch = takeoffPatch();
    const gains = breathGains(patch);
    expect(gains).toHaveLength(3);
    expect(gains[0]!).toBeGreaterThan(gains[1]!);
    expect(gains[1]!).toBeGreaterThan(gains[2]!);
    expect(patch.voices.some((v) => v.wave === "sine")).toBe(false);
    expect(patchLength(patch)).toBeLessThanOrEqual(4.5);
  });

  it("fades in over three breaths on the landing, then thumps", () => {
    const patch = landingPatch();
    const gains = breathGains(patch);
    expect(gains).toHaveLength(3);
    expect(gains[0]!).toBeLessThan(gains[1]!);
    expect(gains[1]!).toBeLessThan(gains[2]!);
    const thump = patch.voices.at(-1)!;
    expect(thump.wave).toBe("sine");
    expect(firstHz(thump)).toBeCloseTo(90, 0);
    expect(thump.pitch.at(-1)!.value).toBeCloseTo(40, 0);
    // After the last breath.
    const lastBreath = Math.max(
      ...breaths(patch)
        .flat()
        .map((v) => v.at),
    );
    expect(thump.at).toBeGreaterThan(lastBreath + 1);
    expect(patchLength(patch)).toBeLessThanOrEqual(4.5);
  });

  it("sweeps each breath's band from its own start", () => {
    // Mutation caught: a later breath's sweep written from the patch
    // start (over before the breath is heard).
    for (const patch of [takeoffPatch(), landingPatch()]) {
      for (const vs of breaths(patch)) {
        for (const v of vs) {
          expect(v.filter?.cutoff[0]?.at).toBeCloseTo(v.at, 9);
          expect(v.filter?.q).toBe(8);
        }
      }
    }
  });
});

describe("answers (M4 C25)", () => {
  const canned: RoomSpec[] = [
    generateRoom(CANNED_BRIDGE),
    generateRoom(CANNED_WORKSHOP),
    generateRoom(CANNED_HUB),
    galleryRoom(),
    heroHallRoom(),
  ];

  // Mutation caught: a rate far off 1 in 48.
  it("picks about one terminal in 48", () => {
    const fixtures: Fixture[] = [
      ...canned.flatMap((r) => r.fixtures.filter((f) => f.kind === "terminal")),
      ...Array.from({ length: 5000 }, (_, i) => terminal(i * 7919 + 13)),
    ];
    const rate = fixtures.filter(answers).length / fixtures.length;
    expect(ANSWER_EVERY).toBe(48);
    expect(rate).toBeGreaterThan(0.015);
    expect(rate).toBeLessThan(0.027);
  });

  it("answers the same for the same fixture, and never for a door", () => {
    for (let seed = 0; seed < 500; seed++) {
      const f = terminal(seed);
      expect(answers(f)).toBe(answers({ ...f }));
    }
    const door = canned
      .flatMap((r) => r.fixtures)
      .find((f) => f.kind === "door");
    if (door === undefined) throw new Error("no door");
    const answering = Array.from({ length: 5000 }, (_, i) => i).find((seed) =>
      answers(terminal(seed)),
    );
    expect(answering).toBeDefined();
    expect(answers({ ...door, seed: answering! })).toBe(false);
  });
});
