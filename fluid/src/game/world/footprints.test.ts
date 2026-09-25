import { describe, expect, it } from "vitest";

import { FOOTPRINTS, MAX_FLOOR_PROP, propFootprint } from "./footprints";
import footprintsSource from "./footprints.ts?raw";
import unitsSource from "./units.ts?raw";
import type { Prop } from "./types";

/** Every module specifier a source file imports or re-exports from. */
function specifiersOf(source: string): string[] {
  const out: string[] = [];
  for (const m of source.matchAll(/\bfrom\s+"([^"]+)"/g)) {
    if (m[1] !== undefined) out.push(m[1]);
  }
  for (const m of source.matchAll(/\bimport\s+"([^"]+)"/g)) {
    if (m[1] !== undefined) out.push(m[1]);
  }
  return out;
}

describe("the leaf modules of the world", () => {
  // The generator reads footprints and the walking code reads the scaffold
  // the generator made, so `footprints.ts` and `units.ts` must stay below
  // both: an import of either one from here would close a cycle.
  it("keeps units.ts free of any import", () => {
    expect(specifiersOf(unitsSource)).toEqual([]);
  });

  it("lets footprints.ts import only units.ts and types.ts", () => {
    const specs = specifiersOf(footprintsSource);
    expect(specs.length).toBeGreaterThan(0);
    for (const s of specs) expect(["./units", "./types"]).toContain(s);
  });

  it("never reaches generate.ts or move.ts from either leaf", () => {
    for (const source of [unitsSource, footprintsSource]) {
      for (const s of specifiersOf(source)) {
        expect(s).not.toMatch(/(^|\/)(generate|move)(\.ts)?$/);
      }
    }
  });
});

describe("MAX_FLOOR_PROP", () => {
  it("keeps every floor prop within MAX_FLOOR_PROP either way", () => {
    // A prop centred in a 2 m corner cell then stands 0.3 m off each wall,
    // clear of the wall band (FLUSH_DEPTH 0.3), and two mid-hall clusters one
    // cell apart stay 2.6 m apart, past the 1.0 m ring (E4).
    expect(MAX_FLOOR_PROP).toBe(1.4);
    for (const [kind, sizes] of Object.entries(FOOTPRINTS.prop))
      for (const [v, s] of sizes.entries()) {
        expect(s.width, `${kind} ${String(v)}`).toBeLessThanOrEqual(
          MAX_FLOOR_PROP,
        );
        expect(s.depth, `${kind} ${String(v)}`).toBeLessThanOrEqual(
          MAX_FLOOR_PROP,
        );
      }
  });
});

describe("propFootprint", () => {
  it("centres a crate's footprint on its cell point, at turn 0", () => {
    const crate: Prop = {
      kind: "crate",
      variant: 1,
      anchor: "floor",
      x: 3.5,
      y: 2.5,
      turn: 0,
      seed: 0,
    };
    expect(propFootprint(crate)).toEqual({
      x0: 6.4,
      x1: 7.6,
      z0: 4.4,
      z1: 5.6,
    });
  });

  it("swaps width and depth at a quarter or three-quarter turn, for a bench", () => {
    const facingNorth: Prop = {
      kind: "bench",
      variant: 0,
      anchor: "floor",
      x: 5,
      y: 5,
      turn: 0,
      seed: 0,
    };
    const box0 = propFootprint(facingNorth);
    if (box0 === null) throw new Error("expected a footprint");
    for (const turn of [1, 3]) {
      const turned: Prop = { ...facingNorth, turn };
      const box = propFootprint(turned);
      if (box === null) throw new Error("expected a footprint");
      expect(box.x1 - box.x0).toBeCloseTo(box0.z1 - box0.z0);
      expect(box.z1 - box.z0).toBeCloseTo(box0.x1 - box0.x0);
    }
  });

  it("gives null for a wall or ceiling prop", () => {
    const wall: Prop = {
      kind: "sign-plate",
      variant: 0,
      anchor: "wall",
      x: 0,
      y: 0,
      turn: 0,
      seed: 0,
    };
    const ceiling: Prop = {
      kind: "beacon",
      variant: 0,
      anchor: "ceiling",
      x: 0,
      y: 0,
      turn: 0,
      seed: 0,
    };
    expect(propFootprint(wall)).toBeNull();
    expect(propFootprint(ceiling)).toBeNull();
  });

  it("throws on a variant index out of range", () => {
    const outOfRange: Prop = {
      kind: "crate",
      variant: 5,
      anchor: "floor",
      x: 0,
      y: 0,
      turn: 0,
      seed: 0,
    };
    expect(() => propFootprint(outOfRange)).toThrow(/no variant 5/);
  });

  it("names the kind when a floor-anchored prop's kind has no floor sizes", () => {
    const misplaced: Prop = {
      kind: "duct",
      variant: 0,
      anchor: "floor",
      x: 0,
      y: 0,
      turn: 0,
      seed: 0,
    };
    expect(() => propFootprint(misplaced)).toThrow(
      /propFootprint: duct has no variant 0/,
    );
  });
});
