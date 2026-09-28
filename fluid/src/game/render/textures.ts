/**
 * The procedural layers, made once at start from nothing but a seed.
 *
 * In the demoscene manner there are no image files: panels are value noise
 * over a bevelled grid, the floor is riveted plates, the ceiling light
 * panels, metal is brushed noise, hazard is diagonal stripes, the portal is
 * a swirl the shader scrolls, grime is fractal noise the shader lays over
 * any surface by the look's grime amount, ribbed is six vertical ribs with
 * a groove at mid-height, and plated is one riveted plate (2.7 C11). The
 * decal layer is the decal atlas (`decalLayer`, 2.7 C20): a 4 by 4 sheet
 * of tiles whose shapes live in their alpha alone, which the scene shader
 * tests against an ordered threshold. Each is a plain RGBA byte array so
 * it can be tested without a GPU and uploaded as one layer of the texture
 * array.
 *
 * Surfaces are generated mostly light and grey; colour comes from the look's
 * tint in the shader, which is what lets one set of layers serve every look.
 */

import { createRng, type Rng } from "../core/seed";
import type { DecalKind } from "../world/types";
import { LAYER } from "./layers";

/** RGBA bytes of one square layer. */
export type Pixels = Uint8Array;

/** Tileable value noise in [0, 1], `cells` lattice cells a side. */
export function valueNoise(
  size: number,
  rng: Rng,
  cells: number,
): Float32Array {
  const lattice = Array.from({ length: cells * cells }, () => rng.next());
  const at = (x: number, y: number) =>
    lattice[((y + cells) % cells) * cells + ((x + cells) % cells)] ?? 0;
  const out = new Float32Array(size * size);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const fx = (x / size) * cells;
      const fy = (y / size) * cells;
      const ix = Math.floor(fx);
      const iy = Math.floor(fy);
      const tx = fx - ix;
      const ty = fy - iy;
      const sx = tx * tx * (3 - 2 * tx);
      const sy = ty * ty * (3 - 2 * ty);
      const top = at(ix, iy) * (1 - sx) + at(ix + 1, iy) * sx;
      const bottom = at(ix, iy + 1) * (1 - sx) + at(ix + 1, iy + 1) * sx;
      out[y * size + x] = top * (1 - sy) + bottom * sy;
    }
  }
  return out;
}

/** Fractal (several octaves of) value noise in [0, 1]. */
function fbm(size: number, rng: Rng, octaves: number): Float32Array {
  const out = new Float32Array(size * size);
  let amplitude = 0.5;
  let total = 0;
  for (let o = 0; o < octaves; o++) {
    const n = valueNoise(size, rng, 4 << o);
    for (let i = 0; i < out.length; i++)
      out[i] = (out[i] ?? 0) + (n[i] ?? 0) * amplitude;
    total += amplitude;
    amplitude /= 2;
  }
  for (let i = 0; i < out.length; i++) out[i] = (out[i] ?? 0) / total;
  return out;
}

function layer(
  size: number,
  shade: (x: number, y: number) => [number, number, number],
): Pixels {
  const out = new Uint8Array(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const [r, g, b] = shade(x, y);
      const o = (y * size + x) * 4;
      out[o] = Math.max(0, Math.min(255, Math.round(r)));
      out[o + 1] = Math.max(0, Math.min(255, Math.round(g)));
      out[o + 2] = Math.max(0, Math.min(255, Math.round(b)));
      out[o + 3] = 255;
    }
  }
  return out;
}

/**
 * True within `width` texels of any of the layer's four edges: the ribbed
 * and the plated pattern both darken there all round, so every wrap (2.7
 * C11: the ribbed pattern's every 0.5 m along and 2 m up, the plated
 * pattern's every 2 m along and 1 m up) reads as a seam.
 */
function nearLayerEdge(x: number, y: number, size: number, width: number) {
  return x < width || x >= size - width || y < width || y >= size - width;
}

/**
 * True at the centre of one of the plated layer's edge rivets: a 3 by 3
 * texel mark every `size / 16` texels along each edge, its centre 8 texels
 * in from that edge (2.7 C11).
 */
function isRivet(x: number, y: number, size: number): boolean {
  const period = size / 16;
  const near = (v: number, c: number) => Math.abs(v - c) <= 1;
  const centres = Array.from({ length: size / period }, (_, k) =>
    Math.floor(k * period + period / 2),
  );
  const onRow = (row: number) =>
    near(y, row) && centres.some((c) => near(x, c));
  const onColumn = (column: number) =>
    near(x, column) && centres.some((c) => near(y, c));
  return (
    onRow(8) || onRow(size - 1 - 8) || onColumn(8) || onColumn(size - 1 - 8)
  );
}

/**
 * Layers 0 to `LAYER.decal`: every procedural layer, with `pictogram` a
 * blank the browser draws its pictograms over and `decal` the decal atlas
 * (`decalLayer`).
 */
export function baseLayers(size: number, seed: number): Pixels[] {
  const rng = createRng(seed);
  const grain = fbm(size, rng, 3);
  const g = (x: number, y: number) => grain[y * size + x] ?? 0;
  const edge = (x: number, y: number, period: number, width: number) => {
    const px = x % period;
    const py = y % period;
    return Math.min(px, py, period - 1 - px, period - 1 - py) < width;
  };

  const panel = layer(size, (x, y) => {
    const bevel = edge(x, y, size / 2, 2) ? -40 : 0;
    const v = 225 + (g(x, y) - 0.5) * 30 + bevel;
    return [v, v, v];
  });
  const floor = layer(size, (x, y) => {
    const plate = edge(x, y, size / 2, 1) ? -60 : 0;
    const rivet = x % (size / 4) < 3 && y % (size / 4) < 3 ? 40 : 0;
    const v = 200 + (g(x, y) - 0.5) * 50 + plate + rivet;
    return [v, v, v];
  });
  const ceiling = layer(size, (x, y) => {
    const v = 235 + (edge(x, y, size / 4, 1) ? -50 : 0) + (g(x, y) - 0.5) * 16;
    return [v, v, v];
  });
  const brushed = valueNoise(size, rng, 64);
  const metal = layer(size, (x, y) => {
    const v =
      190 +
      ((brushed[y * size + ((x * 7) % size)] ?? 0) - 0.5) * 40 +
      (g(x, y) - 0.5) * 20;
    return [v, v, v];
  });
  const hazard = layer(size, (x, y) =>
    Math.floor((x + y) / (size / 8)) % 2 === 0 ? [240, 190, 20] : [30, 28, 24],
  );
  const swirlNoise = fbm(size, rng, 4);
  const portal = layer(size, (x, y) => {
    const dx = x / size - 0.5;
    const dy = y / size - 0.5;
    const angle = Math.atan2(dy, dx);
    const radius = Math.hypot(dx, dy);
    const swirl =
      0.5 +
      0.5 *
        Math.sin(angle * 3 + radius * 18 + (swirlNoise[y * size + x] ?? 0) * 6);
    const v = 120 + swirl * 135;
    return [v, v, v];
  });
  const grimeNoise = fbm(size, rng, 5);
  const grime = layer(size, (x, y) => {
    const v = Math.pow(grimeNoise[y * size + x] ?? 0, 1.5) * 255;
    return [v, v, v];
  });
  const pictogram = layer(size, () => [255, 255, 255]);
  const ribbed = layer(size, (x, y) => {
    // The cosine is phase-shifted (a leading minus) so a trough, not a
    // peak, sits on the x wrap at 0 and size: the edge darkening below
    // would otherwise cut a rib centred there into two half ribs.
    let v =
      212 - 26 * Math.cos((2 * Math.PI * 6 * x) / size) + (g(x, y) - 0.5) * 20;
    if (Math.abs(y - size / 2) <= 1) v -= 45;
    if (nearLayerEdge(x, y, size, 2)) v -= 40;
    return [v, v, v];
  });
  const plated = layer(size, (x, y) => {
    let v = 226 + (g(x, y) - 0.5) * 24;
    if (nearLayerEdge(x, y, size, 2)) v -= 40;
    if (isRivet(x, y, size)) v = 250;
    return [v, v, v];
  });
  const decal = decalLayer(size);

  const out: Pixels[] = [];
  out[LAYER.panel] = panel;
  out[LAYER.floor] = floor;
  out[LAYER.ceiling] = ceiling;
  out[LAYER.metal] = metal;
  out[LAYER.hazard] = hazard;
  out[LAYER.portal] = portal;
  out[LAYER.grime] = grime;
  out[LAYER.pictogram] = pictogram;
  out[LAYER.ribbed] = ribbed;
  out[LAYER.plated] = plated;
  out[LAYER.decal] = decal;
  return out;
}

/** How many tiles the decal atlas has a side (2.7 C20). */
const ATLAS_TILES = 4;

/**
 * The side of a decal tile in the atlas's own texel units, and its clear
 * margin (2.7 C20): at the 256-texel layer a tile is 64 texels and its
 * outer 4 texels on every side are fully transparent, so a coarser mip
 * level never bleeds one tile's shape into its neighbour. A layer of
 * another size scales the whole sheet, margin and all, sampling each
 * texel at its centre: the margin keeps its 4 texels at 256, is 2 at 128
 * and 1 at 64, and is lost below that (at 32 a tile's first texel already
 * falls inside it). The station draws the atlas at `LAYER_SIZE` (256)
 * only; a smaller layer is for tests of the other layers.
 */
const TILE = 64;
const MARGIN = 4;

/**
 * The decal atlas's tiles by kind (2.7 C20), row by row from the layer's
 * first row: chevrons 0 and 1, the arrow 2, grime 3 to 5, streaks 6 and 7,
 * rust 8 and 9, and the solid tile 15, whose alpha is full inside its
 * margin, for the stencils' pixels (a stencil draws one point of `solid`
 * and has no entry of its own). Tiles 10 to 14 are empty. A decal's `variant`
 * indexes its kind's list. The airlock's hazard ring (M3 C24) draws the
 * chevrons' tiles and the solid tile and has no entry of its own either.
 */
export const DECAL_TILES: Readonly<
  Record<Exclude<DecalKind, "stencil" | "ring"> | "solid", readonly number[]>
> = {
  chevrons: [0, 1],
  arrow: [2],
  grime: [3, 4, 5],
  streak: [6, 7],
  rust: [8, 9],
  solid: [15],
};

/**
 * A window of a decal tile as a uv rectangle of the layer: the tile's
 * texels `from` to `to` both ways, in the 64-texel tile's own units. A
 * byte layer is uploaded as it is (only canvas layers are flipped), so
 * `v` 0 is the layer's first row and a tile's high `v` is its top.
 */
export function tileWindow(
  tile: number,
  from: number,
  to: number,
): { u0: number; v0: number; u1: number; v1: number } {
  const col = tile % ATLAS_TILES;
  const row = Math.floor(tile / ATLAS_TILES);
  const at = (cell: number, texel: number) =>
    (cell * TILE + texel) / (TILE * ATLAS_TILES);
  return {
    u0: at(col, from),
    v0: at(row, from),
    u1: at(col, to),
    v1: at(row, to),
  };
}

/**
 * A decal tile's uv rectangle (2.7 C20): the tile inside its clear
 * margin, `u0` to `u1` left to right and `v0` to `v1` bottom to top, so a
 * quad mapped onto it shows the tile's whole shape and none of the
 * margin's clear texels at its edges.
 */
export function tileRect(tile: number): {
  u0: number;
  v0: number;
  u1: number;
  v1: number;
} {
  return tileWindow(tile, MARGIN, TILE - MARGIN);
}

/** `x` eased from 0 at `a` to 1 at `b`, as GLSL's `smoothstep`. */
function smooth(a: number, b: number, x: number): number {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
}

/** A tile's alpha, 0 to 1, at texel `(x, y)` of its 64 by 64 (y 0 its bottom). */
type TileShape = (x: number, y: number) => number;

/**
 * Hazard stripes (2.7 C20): bars 16 texels wide at 45 degrees, one way
 * (`mirror` false) or the other, hard edged. The pattern repeats every 32
 * texels both ways, so any 32 by 32 window of it tiles seamlessly.
 */
const chevrons =
  (mirror: boolean): TileShape =>
  (x, y) =>
    (((mirror ? x - y + TILE : x + y) % 32) + 32) % 32 < 16 ? 1 : 0;

/** An arrow pointing to the tile's top: a shaft in rows 4 to 31, a head in 32 to 59, hard edged. */
const arrow: TileShape = (x, y) => {
  const cx = x + 0.5 - TILE / 2;
  if (y < 32) return Math.abs(cx) < 6 ? 1 : 0;
  const half = (26 * (TILE - MARGIN - 0.5 - y)) / (TILE - MARGIN - 0.5 - 32);
  return Math.abs(cx) < half ? 1 : 0;
};

/**
 * Fractal noise over one tile, two octaves of `valueNoise`, 0 to 1: the
 * texture of grime and of a rust blotch.
 */
function tileNoise(rng: Rng): (x: number, y: number) => number {
  const a = valueNoise(TILE, rng, 4);
  const b = valueNoise(TILE, rng, 8);
  const c = valueNoise(TILE, rng, 16);
  return (x, y) => {
    const i = y * TILE + x;
    return ((a[i] ?? 0) * 4 + (b[i] ?? 0) * 2 + (c[i] ?? 0)) / 7;
  };
}

/**
 * A grime blob: fractal noise, strongest in the tile's middle and falling
 * to nothing well before its margin, soft edged, so the shader's ordered
 * threshold stipples it.
 */
function grime(rng: Rng): TileShape {
  const n = tileNoise(rng);
  return (x, y) => {
    const r =
      Math.hypot(x + 0.5 - TILE / 2, y + 0.5 - TILE / 2) / (TILE / 2 - MARGIN);
    const fall = 1 - smooth(0.4, 0.92, r);
    return Math.min(1, Math.max(0, (n(x, y) * fall - 0.18) * 2.4));
  };
}

/**
 * Drips hanging from the tile's top (its high rows): a soft band along the
 * top and five to seven drips of their own width and length below it, each
 * strongest where it leaves the top and fading along its length.
 */
function drips(rng: Rng): TileShape {
  const top = TILE - MARGIN - 1;
  const count = rng.int(5, 7);
  const list = Array.from({ length: count }, () => ({
    x: rng.range(12, TILE - 12),
    w: rng.range(1.5, 4.5),
    len: rng.range(18, 50),
  }));
  return (x, y) => {
    const cx = x + 0.5;
    const across =
      1 - smooth(TILE / 2 - 12, TILE / 2 - MARGIN - 1, Math.abs(cx - TILE / 2));
    const band = smooth(top - 10, top, y) * 0.7 * across;
    let a = band;
    for (const d of list) {
      const down = top - y;
      if (down < 0 || down > d.len) continue;
      const g = Math.exp(-(((cx - d.x) / d.w) ** 2));
      a = Math.max(a, g * Math.pow(1 - down / d.len, 0.6) * 0.95);
    }
    return a;
  };
}

/** Rust: drips as a streak's, under a noisy blotch at the tile's top. */
function rust(rng: Rng): TileShape {
  const drip = drips(rng);
  const n = tileNoise(rng);
  const cx = rng.range(26, 38);
  const cy = TILE - MARGIN - 9;
  return (x, y) => {
    const r = Math.hypot(x + 0.5 - cx, (y + 0.5 - cy) * 1.6) / 14;
    const blotch = (1 - smooth(0.45, 1, r)) * (0.55 + 0.45 * n(x, y));
    return Math.max(drip(x, y), blotch);
  };
}

/**
 * The decal atlas (2.7 C20), `size` texels a side: a 4 by 4 sheet of
 * tiles, each shape drawn in its tile's alpha with a clear margin all
 * round (`DECAL_TILES` names the tiles): two sets of hazard stripes, the
 * arrow (pointing to its tile's top), three grime blobs, two streaks of
 * drips from the top, two rust bleeds (drips under a blotch) and the solid
 * tile. The chevrons, the arrow and the solid tile are hard edged, alpha 0
 * or 1, so the shader's alpha test keeps them crisp; grime, streaks and
 * rust fade softly, so the test stipples them. Every colour byte is white
 * (255): a decal's colour is its tint, which the texture then leaves
 * alone in every look. The same size gives the same bytes; the atlas draws
 * from a fixed seed of its own.
 */
export function decalLayer(size: number): Pixels {
  const rng = createRng(0xdeca1);
  const shapes = new Map<number, TileShape>([
    [0, chevrons(false)],
    [1, chevrons(true)],
    [2, arrow],
    [3, grime(rng)],
    [4, grime(rng)],
    [5, grime(rng)],
    [6, drips(rng)],
    [7, drips(rng)],
    [8, rust(rng)],
    [9, rust(rng)],
    [15, () => 1],
  ]);
  const tile = size / ATLAS_TILES;
  const out = new Uint8Array(size * size * 4).fill(255);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const t = Math.floor(y / tile) * ATLAS_TILES + Math.floor(x / tile);
      // The texel's place in its tile, in the 64-texel tile's units.
      const tx = Math.floor(((x % tile) + 0.5) * (TILE / tile));
      const ty = Math.floor(((y % tile) + 0.5) * (TILE / tile));
      const margin =
        tx < MARGIN ||
        ty < MARGIN ||
        tx >= TILE - MARGIN ||
        ty >= TILE - MARGIN;
      const shape = shapes.get(t);
      const a = margin || shape === undefined ? 0 : shape(tx, ty);
      out[(y * size + x) * 4 + 3] = Math.round(
        Math.min(1, Math.max(0, a)) * 255,
      );
    }
  }
  return out;
}
