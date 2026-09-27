import { describe, expect, it } from "vitest";

import { createRng } from "../core/seed";
import { LAYER, LAYER_SIZE } from "./layers";
import { baseLayers, valueNoise } from "./textures";

describe("baseLayers", () => {
  const layers = baseLayers(32, 1);

  it("makes one RGBA layer per base slot, pictogram and decal placeholders included", () => {
    expect(layers).toHaveLength(LAYER.decal + 1);
    for (const l of layers) expect(l.length).toBe(32 * 32 * 4);
  });

  it("is opaque everywhere but the decal layer", () => {
    for (const l of layers.slice(0, LAYER.decal)) {
      for (let i = 3; i < l.length; i += 4) expect(l[i]).toBe(255);
    }
  });

  it("is the same for the same seed", () => {
    expect(baseLayers(32, 1)).toEqual(layers);
  });

  it("draws six ribs across the ribbed layer and a groove at its middle (2.7 C11)", () => {
    // Mutation caught: the ribbed layer a copy of the panel layer, the ribs'
    // count or the groove missing.
    const layers = baseLayers(LAYER_SIZE, 1);
    const ribbed = layers[LAYER.ribbed];
    const panel = layers[LAYER.panel];
    expect(ribbed).toBeDefined();
    expect(ribbed).not.toEqual(panel);
    const at = (x: number, y: number) => ribbed![(y * LAYER_SIZE + x) * 4] ?? 0;
    // Local maxima of a row a quarter up, smoothed over 3 texels.
    const y = LAYER_SIZE / 4;
    const row = Array.from(
      { length: LAYER_SIZE },
      (_, x) =>
        (at((x + LAYER_SIZE - 1) % LAYER_SIZE, y) +
          at(x, y) +
          at((x + 1) % LAYER_SIZE, y)) /
        3,
    );
    const peaks = row.filter(
      (v, x) =>
        v > (row[(x + LAYER_SIZE - 1) % LAYER_SIZE] ?? 0) &&
        v >= (row[(x + 1) % LAYER_SIZE] ?? 0),
    ).length;
    expect(peaks).toBe(6);
    const mid = LAYER_SIZE / 2;
    // Floored: a fractional texel index always misses the typed array
    // (`?? 0` on both sides), which would make this assertion unwinnable
    // whatever the layer holds.
    const third = Math.floor(LAYER_SIZE / 3);
    expect(at(third, mid)).toBeLessThan(at(third, y) - 20);
  });

  it("rivets the plated layer's edges and leaves its middle plain (2.7 C11)", () => {
    // Mutation caught: no rivets, rivets in the middle, or a copy of the
    // panel layer.
    const plated = baseLayers(LAYER_SIZE, 1)[LAYER.plated];
    expect(plated).toBeDefined();
    const at = (x: number, y: number) => plated![(y * LAYER_SIZE + x) * 4] ?? 0;
    const edgeRow = Array.from({ length: LAYER_SIZE }, (_, x) => at(x, 8));
    const midRow = Array.from({ length: LAYER_SIZE }, (_, x) =>
      at(x, LAYER_SIZE / 2),
    );
    const bright = (r: number[]) => r.filter((v) => v > 245).length;
    expect(bright(edgeRow)).toBeGreaterThanOrEqual(16);
    expect(bright(midRow)).toBe(0);
  });

  it("leaves the decal layer transparent until the atlas is drawn", () => {
    // Mutation caught: the decal layer opaque (every decal a solid square).
    const decal = baseLayers(LAYER_SIZE, 1)[LAYER.decal];
    expect(decal).toBeDefined();
    expect(decal!.every((v, i) => i % 4 !== 3 || v === 0)).toBe(true);
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
