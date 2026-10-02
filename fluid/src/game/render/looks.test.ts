import { describe, expect, it } from "vitest";

import { ACCENT_COUNT } from "../world/variants";
import { LOOK, applyCondition, hueToRgb, type Look, type Rgb } from "./looks";
import type { Condition } from "../world/types";

function allColours(look: Look): Rgb[] {
  return [...Object.values(look.palette), look.edge.colour, ...look.accents];
}

describe("the look", () => {
  it("keeps every colour inside [0, 1]", () => {
    const colours = allColours(LOOK);
    expect(colours.length).toBeGreaterThan(0);
    for (const c of colours) {
      for (const v of c) {
        expect(v).toBeGreaterThanOrEqual(0);
        expect(v).toBeLessThanOrEqual(1);
      }
    }
  });

  it("puts glowing seams on the shell", () => {
    expect(LOOK.edge.everywhere).toBe(true);
  });

  it("makes the doors cyan and the portals orange", () => {
    const [dr, dg, db] = LOOK.palette.door;
    expect(dg).toBeGreaterThan(dr);
    expect(db).toBeGreaterThan(dr);
    const [pr, pg, pb] = LOOK.palette.portal;
    expect(pr).toBeGreaterThan(pg);
    expect(pg).toBeGreaterThan(pb);
  });

  it("has five accents, clear of the colours that mark a way (2.7 C7)", () => {
    // Mutation caught: an accent too near a door, portal or cross-domain
    // portal colour (an accent would read as a way), or a look short of one.
    const dist = (a: Rgb, b: Rgb) =>
      Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    expect(LOOK.accents).toHaveLength(ACCENT_COUNT);
    for (const a of LOOK.accents)
      for (const way of [
        LOOK.palette.door,
        LOOK.palette.portal,
        LOOK.palette.portalAlt,
      ])
        expect(dist(a, way)).toBeGreaterThanOrEqual(0.25);
  });
});

describe("applyCondition", () => {
  it("leaves a clean room alone", () => {
    expect(applyCondition(LOOK, "clean")).toEqual(LOOK);
  });

  it("makes a retired room grimier and darker, a derelict one more so", () => {
    const dim = applyCondition(LOOK, "dim");
    const derelict = applyCondition(LOOK, "derelict");
    expect(dim.grime).toBeGreaterThan(LOOK.grime);
    expect(derelict.grime).toBeGreaterThan(dim.grime);
    expect(dim.lightScale).toBeLessThan(LOOK.lightScale);
    expect(derelict.lightScale).toBeLessThan(dim.lightScale);
  });

  it("does not change the look it was given", () => {
    const before = JSON.stringify(LOOK);
    applyCondition(LOOK, "derelict");
    expect(JSON.stringify(LOOK)).toBe(before);
  });

  it("leaves the palette alone in every condition, so prop meshes are cached by look", () => {
    const conditions = [
      "clean",
      "construction",
      "dim",
      "derelict",
    ] satisfies readonly Condition[];
    for (const c of conditions) {
      expect(applyCondition(LOOK, c).palette).toBe(LOOK.palette);
    }
  });
});

describe("hueToRgb", () => {
  it("gives pure red at hue 0 with full saturation", () => {
    const [r, g, b] = hueToRgb(0, 1, 0.5);
    expect(r).toBeCloseTo(1);
    expect(g).toBeCloseTo(0);
    expect(b).toBeCloseTo(0);
  });
});
