import { describe, expect, it } from "vitest";

import { ACCENT_COUNT } from "../world/variants";
import {
  C64_PALETTE,
  LOOKS,
  LOOK_ORDER,
  applyCondition,
  hueToRgb,
  lookForKey,
  type Rgb,
} from "./looks";
import type { Condition } from "../world/types";

function allColours(look: (typeof LOOKS)["day"]): Rgb[] {
  return [...Object.values(look.palette), look.edge.colour, ...look.accents];
}

describe("looks", () => {
  it("are the three the spec names, on keys 1, 2 and 4", () => {
    expect(LOOK_ORDER).toEqual(["day", "aperture", "freescape"]);
    expect(LOOKS.day.name).toBe("Day shift");
    expect(LOOKS.aperture.name).toBe("Aperture grid");
    expect(LOOKS.freescape.name).toBe("Freescape 64");
    expect(lookForKey("Digit1")).toBe("day");
    expect(lookForKey("Digit2")).toBe("aperture");
    expect(lookForKey("Digit4")).toBe("freescape");
    expect(lookForKey("Digit3")).toBeNull();
    expect(lookForKey("KeyW")).toBeNull();
  });

  it("keep every colour inside [0, 1]", () => {
    for (const id of LOOK_ORDER) {
      for (const c of allColours(LOOKS[id])) {
        for (const v of c) {
          expect(v).toBeGreaterThanOrEqual(0);
          expect(v).toBeLessThanOrEqual(1);
        }
      }
    }
  });

  it("put glowing seams on the shell only in Aperture grid", () => {
    expect(LOOKS.day.edge.everywhere).toBe(false);
    expect(LOOKS.aperture.edge.everywhere).toBe(true);
  });

  it("make Aperture grid's doors cyan and portals orange", () => {
    const [dr, dg, db] = LOOKS.aperture.palette.door;
    expect(dg).toBeGreaterThan(dr);
    expect(db).toBeGreaterThan(dr);
    const [pr, pg, pb] = LOOKS.aperture.palette.portal;
    expect(pr).toBeGreaterThan(pg);
    expect(pg).toBeGreaterThan(pb);
  });

  it("make Freescape 64 flat, dithered and drawn in C64 colours only", () => {
    const f = LOOKS.freescape;
    expect(f.flat).toBe(true);
    expect(f.dither).toBe(true);
    expect(f.textureMix).toBe(0);
    const inPalette = (c: Rgb) =>
      C64_PALETTE.some((p) =>
        p.every((v, i) => Math.abs(v - (c[i] ?? -1)) < 1e-6),
      );
    for (const c of Object.values(f.palette)) expect(inPalette(c)).toBe(true);
    for (const c of f.accents) expect(inPalette(c)).toBe(true);
  });

  it("gives every look five accents, clear of the colours that mark a way (2.7 C7)", () => {
    // Mutation caught: an accent too near a door, portal or cross-domain
    // portal colour (an accent would read as a way), or a look short of one.
    const dist = (a: Rgb, b: Rgb) =>
      Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    for (const id of LOOK_ORDER) {
      const look = LOOKS[id];
      expect(look.accents).toHaveLength(ACCENT_COUNT);
      for (const a of look.accents)
        for (const way of [
          look.palette.door,
          look.palette.portal,
          look.palette.portalAlt,
        ])
          expect(dist(a, way)).toBeGreaterThanOrEqual(0.25);
    }
  });

  it("have exactly sixteen C64 colours", () => {
    expect(C64_PALETTE).toHaveLength(16);
  });
});

describe("applyCondition", () => {
  it("leaves a clean room alone", () => {
    expect(applyCondition(LOOKS.day, "clean")).toEqual(LOOKS.day);
  });

  it("makes a retired room grimier and darker, a derelict one more so", () => {
    const dim = applyCondition(LOOKS.day, "dim");
    const derelict = applyCondition(LOOKS.day, "derelict");
    expect(dim.grime).toBeGreaterThan(LOOKS.day.grime);
    expect(derelict.grime).toBeGreaterThan(dim.grime);
    expect(dim.lightScale).toBeLessThan(LOOKS.day.lightScale);
    expect(derelict.lightScale).toBeLessThan(dim.lightScale);
  });

  it("does not change the look it was given", () => {
    const before = JSON.stringify(LOOKS.aperture);
    applyCondition(LOOKS.aperture, "derelict");
    expect(JSON.stringify(LOOKS.aperture)).toBe(before);
  });

  it("leaves the palette alone in every condition, so prop meshes are cached by look", () => {
    const conditions = [
      "clean",
      "construction",
      "dim",
      "derelict",
    ] satisfies readonly Condition[];
    for (const id of LOOK_ORDER) {
      for (const c of conditions) {
        expect(applyCondition(LOOKS[id], c).palette).toBe(LOOKS[id].palette);
      }
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
