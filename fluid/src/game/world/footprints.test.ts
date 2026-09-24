import { describe, expect, it } from "vitest";

import footprintsSource from "./footprints.ts?raw";
import unitsSource from "./units.ts?raw";

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
