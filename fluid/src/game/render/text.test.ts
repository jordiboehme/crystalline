import { describe, expect, it } from "vitest";

import { PROP_CATALOGUE } from "../world/props";
import { LAYER_SIZE } from "./layers";
import { LOOK_ORDER, LOOKS } from "./looks";
import { colours, flipRows, PICTOGRAM, SIGN_PICTOGRAMS } from "./text";

describe("flipRows", () => {
  it("turns the image upside down so row 0 is the bottom, as GL expects", () => {
    // 1 x 2 image: top pixel red, bottom pixel blue.
    const data = new Uint8ClampedArray([255, 0, 0, 255, 0, 0, 255, 255]);
    expect(Array.from(flipRows(data, 1, 2))).toEqual([
      0, 0, 255, 255, 255, 0, 0, 255,
    ]);
  });
});

describe("PICTOGRAM", () => {
  const entries = Object.entries(PICTOGRAM);

  it("keeps every rect inside [0, 1] and square", () => {
    for (const [key, rect] of entries) {
      expect(rect.uw, key).toBeCloseTo(rect.vh, 10);
      expect(rect.u0).toBeGreaterThanOrEqual(0);
      expect(rect.v0).toBeGreaterThanOrEqual(0);
      expect(rect.u0 + rect.uw).toBeLessThanOrEqual(1);
      expect(rect.v0 + rect.vh).toBeLessThanOrEqual(1);
    }
  });

  it("lands every edge on a whole texel at LAYER_SIZE", () => {
    for (const [key, rect] of entries) {
      for (const edge of [
        rect.u0,
        rect.v0,
        rect.u0 + rect.uw,
        rect.v0 + rect.vh,
      ]) {
        expect(Number.isInteger(edge * LAYER_SIZE), key).toBe(true);
      }
    }
  });

  it("never overlaps two rects", () => {
    for (let i = 0; i < entries.length; i++) {
      for (let j = i + 1; j < entries.length; j++) {
        const [aKey, a] = entries[i] ?? [];
        const [bKey, b] = entries[j] ?? [];
        if (!a || !b) continue;
        const apart =
          a.u0 + a.uw <= b.u0 ||
          b.u0 + b.uw <= a.u0 ||
          a.v0 + a.vh <= b.v0 ||
          b.v0 + b.vh <= a.v0;
        expect(apart, `${String(aKey)} vs ${String(bKey)}`).toBe(true);
      }
    }
  });
});

describe("the key pictogram (M3 C20, C24)", () => {
  it("takes the free tile next to caution", () => {
    // Mutation caught: the key tile missing, or moved onto another tile.
    expect(PICTOGRAM.key).toEqual({ u0: 0.25, v0: 0.25, uw: 0.25, vh: 0.25 });
  });
});

describe("colours", () => {
  it("answers a lift panel's colours in every look, ink apart from background", () => {
    // Mutation caught: the `panel` case missing from the switch, which has
    // no default, so it answers undefined.
    expect(LOOK_ORDER.length).toBeGreaterThan(0);
    for (const id of LOOK_ORDER) {
      const { background, ink } = colours("panel", LOOKS[id]);
      expect(background).toHaveLength(3);
      expect(ink).toHaveLength(3);
      expect(ink).not.toEqual(background);
    }
  });
});

describe("SIGN_PICTOGRAMS", () => {
  it("names six distinct keys of PICTOGRAM, apart from service, portal and door", () => {
    expect(SIGN_PICTOGRAMS.length).toBe(6);
    expect(new Set(SIGN_PICTOGRAMS).size).toBe(6);
    for (const key of SIGN_PICTOGRAMS) {
      expect(Object.keys(PICTOGRAM)).toContain(key);
      expect(["service", "portal", "door"]).not.toContain(key);
    }
  });

  it("has one entry per sign-plate variant", () => {
    expect(SIGN_PICTOGRAMS.length).toBe(PROP_CATALOGUE["sign-plate"].variants);
  });
});
