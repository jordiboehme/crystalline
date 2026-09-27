/**
 * The procedural layers, made once at start from nothing but a seed.
 *
 * In the demoscene manner there are no image files: panels are value noise
 * over a bevelled grid, the floor is riveted plates, the ceiling light
 * panels, metal is brushed noise, hazard is diagonal stripes, the portal is
 * a swirl the shader scrolls, grime is fractal noise the shader lays over
 * any surface by the look's grime amount, ribbed is six vertical ribs with
 * a groove at mid-height, and plated is one riveted plate (2.7 C11). The
 * decal layer starts fully transparent; its atlas is drawn separately. Each
 * is a plain RGBA byte array so it can be tested without a GPU and uploaded
 * as one layer of the texture array.
 *
 * Surfaces are generated mostly light and grey; colour comes from the look's
 * tint in the shader, which is what lets one set of layers serve every look.
 */

import { createRng, type Rng } from "../core/seed";
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
 * True within `width` texels of the top or the bottom of the layer (`y`
 * near 0 or `size - 1`): the ribbed pattern darkens there, so its vertical
 * wrap (2.7 C11: a repeat every 2 m up) reads as a seam. Its six ribs tile
 * seamlessly along `x` already (an integer number of cosine periods across
 * the whole layer), so no `x` band is darkened, or a rib centred on the
 * layer's left or right edge would read as two half ribs instead of one.
 */
function nearTopOrBottom(y: number, size: number, width: number) {
  return y < width || y >= size - width;
}

/**
 * True within `width` texels of any of the layer's four edges: the plated
 * pattern's one plate darkens there all round, so both its wraps (2.7 C11:
 * every 2 m along, every 1 m up) read as a seam.
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
 * blank the browser draws its pictograms over and `decal` a blank its
 * atlas is drawn over.
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
    let v =
      212 + 26 * Math.cos((2 * Math.PI * 6 * x) / size) + (g(x, y) - 0.5) * 20;
    if (Math.abs(y - size / 2) <= 1) v -= 45;
    if (nearTopOrBottom(y, size, 2)) v -= 40;
    return [v, v, v];
  });
  const plated = layer(size, (x, y) => {
    let v = 226 + (g(x, y) - 0.5) * 24;
    if (nearLayerEdge(x, y, size, 2)) v -= 40;
    if (isRivet(x, y, size)) v = 250;
    return [v, v, v];
  });
  // Fully transparent (every byte 0, including alpha) until the atlas is
  // drawn onto it.
  const decal: Pixels = new Uint8Array(size * size * 4);

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
