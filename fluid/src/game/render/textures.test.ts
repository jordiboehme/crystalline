import { describe, expect, it } from "vitest";

import { createRng } from "../core/seed";
import { LAYER, LAYER_SIZE } from "./layers";
import {
  DECAL_TILES,
  baseLayers,
  decalLayer,
  tileRect,
  valueNoise,
} from "./textures";

describe("baseLayers", () => {
  const layers = baseLayers(32, 1);

  it("makes one RGBA layer per base slot, the pictogram layer and the decal atlas included", () => {
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

  it("darkens the ribbed layer's four edges, so every wrap reads as a seam (2.7 C11)", () => {
    // Mutation caught: the ribbed layer's edge darkening dropped (its
    // wraps, every 0.5 m along and 2 m up, would show no seam).
    const ribbed = baseLayers(LAYER_SIZE, 1)[LAYER.ribbed];
    expect(ribbed).toBeDefined();
    const at = (x: number, y: number) => ribbed![(y * LAYER_SIZE + x) * 4] ?? 0;
    const mean = (vs: number[]) => vs.reduce((s, v) => s + v, 0) / vs.length;
    const n = LAYER_SIZE;
    // The top and bottom rows against a row a quarter up, texel by texel
    // across (the ribs run up, so a column's shade is the same up it).
    const across = Array.from({ length: n }, (_, x) => x);
    for (const edge of [0, n - 1])
      expect(mean(across.map((x) => at(x, edge)))).toBeLessThan(
        mean(across.map((x) => at(x, n / 4))) - 30,
      );
    // The left and right columns sit in a trough between ribs; the next
    // trough (a sixth across) is the same shade without the darkening.
    const up = Array.from({ length: n / 2 }, (_, i) => i + n / 8);
    const trough = n / 6;
    for (const edge of [0, n - 1])
      expect(mean(up.map((y) => at(edge, y)))).toBeLessThan(
        mean(up.map((y) => at(Math.round(trough), y))) - 30,
      );
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

  it("hands out the decal atlas as the decal layer (2.7 C20)", () => {
    // Mutation caught: the decal layer left blank (no decal is ever drawn)
    // or opaque (every decal a solid square).
    const decal = baseLayers(LAYER_SIZE, 1)[LAYER.decal];
    expect(decal).toEqual(decalLayer(LAYER_SIZE));
    expect(decal!.some((v, i) => i % 4 === 3 && v > 0)).toBe(true);
    expect(decal!.some((v, i) => i % 4 === 3 && v === 0)).toBe(true);
  });

  it("draws every decal tile's shape in its alpha, inside a clear margin (2.7 C20)", () => {
    // Mutation caught: a tile left empty, a tile that fills its margin (mip
    // bleed into its neighbour), or the arrow pointing down.
    const d = decalLayer(LAYER_SIZE);
    const alpha = (x: number, y: number) =>
      d[(y * LAYER_SIZE + x) * 4 + 3] ?? 0;
    const tiles = Object.values(DECAL_TILES).flat();
    expect(tiles.length).toBeGreaterThan(0);
    for (const t of tiles) {
      const x0 = (t % 4) * 64;
      const y0 = Math.floor(t / 4) * 64;
      let inside = 0;
      for (let y = 0; y < 64; y++)
        for (let x = 0; x < 64; x++) {
          const a = alpha(x0 + x, y0 + y);
          const margin = x < 4 || y < 4 || x >= 60 || y >= 60;
          if (margin) expect(a).toBe(0);
          else if (a > 0) inside++;
        }
      expect(inside).toBeGreaterThan(200);
    }
    // The arrow (tile 2) points to the tile's high v. A byte layer is not
    // flipped (`flipRows` is for canvas layers only), so its row 0 is v 0:
    // the head (wide) lies in the upper rows 32 to 59, the shaft (narrow)
    // in rows 4 to 31.
    const width = (row: number) =>
      Array.from({ length: 64 }, (_, x) => alpha(2 * 64 + x, row)).filter(
        (a) => a > 127,
      ).length;
    const widest = (from: number, to: number) =>
      Math.max(...Array.from({ length: to - from }, (_, i) => width(from + i)));
    expect(widest(32, 60)).toBeGreaterThan(2 * widest(4, 32));
  });

  it("names a tile for every kind that reads one, and none for the stencil (2.7 C20)", () => {
    // Mutation caught: a stencil entry the recipe never reads (the stencil
    // draws one point of the solid tile), which a reader could change to
    // no effect.
    expect(Object.keys(DECAL_TILES).sort()).toEqual(
      ["arrow", "chevrons", "grime", "rust", "solid", "streak"].sort(),
    );
  });

  it("maps a tile's rectangle inside its own margin (2.7 C20)", () => {
    // Mutation caught: a rectangle over the whole tile (its margin's clear
    // texels at every decal's edge) or over the wrong tile.
    for (const t of Object.values(DECAL_TILES).flat()) {
      const r = tileRect(t);
      const x0 = (t % 4) * 64;
      const y0 = Math.floor(t / 4) * 64;
      expect(r.u0 * LAYER_SIZE).toBeCloseTo(x0 + 4, 6);
      expect(r.u1 * LAYER_SIZE).toBeCloseTo(x0 + 60, 6);
      expect(r.v0 * LAYER_SIZE).toBeCloseTo(y0 + 4, 6);
      expect(r.v1 * LAYER_SIZE).toBeCloseTo(y0 + 60, 6);
    }
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
