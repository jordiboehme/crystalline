import { describe, expect, it } from "vitest";

import { createRng } from "../core/seed";
import { LAYER } from "./layers";
import { baseLayers, valueNoise } from "./textures";

describe("baseLayers", () => {
  const layers = baseLayers(32, 1);

  it("makes one RGBA layer per base slot, pictogram placeholder included", () => {
    expect(layers).toHaveLength(LAYER.pictogram + 1);
    for (const l of layers) expect(l.length).toBe(32 * 32 * 4);
  });

  it("is opaque everywhere", () => {
    for (const l of layers.slice(0, LAYER.pictogram)) {
      for (let i = 3; i < l.length; i += 4) expect(l[i]).toBe(255);
    }
  });

  it("is the same for the same seed", () => {
    expect(baseLayers(32, 1)).toEqual(layers);
  });

  it("draws hazard stripes in two clearly different tones", () => {
    const hazard = layers[LAYER.hazard];
    const reds = new Set<number>();
    for (let i = 0; i < (hazard?.length ?? 0); i += 4)
      reds.add(hazard?.[i] ?? 0);
    expect(Math.max(...reds) - Math.min(...reds)).toBeGreaterThan(150);
  });

  it("keeps the panel mostly bright so the look's tint carries the colour", () => {
    const panel = layers[LAYER.panel] ?? new Uint8Array();
    let sum = 0;
    for (let i = 0; i < panel.length; i += 4) sum += panel[i] ?? 0;
    expect(sum / (panel.length / 4)).toBeGreaterThan(180);
  });
});

describe("valueNoise", () => {
  it("stays in [0, 1] and tiles", () => {
    const n = valueNoise(16, createRng(3), 4);
    for (const v of n) {
      expect(v).toBeGreaterThanOrEqual(0);
      expect(v).toBeLessThanOrEqual(1);
    }
  });
});
