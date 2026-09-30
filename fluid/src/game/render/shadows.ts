/**
 * Contact shadows: a soft dark patch on the floor under everything that
 * stands on it, shaped by what stands there. Each thing casts one patch
 * (`Caster`): a soft rectangle the size of its footprint, turned with it,
 * or a soft disc (an ellipse in a footprint that is not square) for a
 * round thing such as a barrel, a cone or a round table. A larger footprint
 * gives a lighter patch with a wider soft edge. The patches are
 * painted once per room into a small R8 texture over the grid
 * (`SHADOW_TEXELS` a metre), which the scene shader reads on the floor and
 * darkens its light by, before the bands, as far as the look's
 * `contactShadow` says (0 in the looks without them).
 *
 * The footprints are the ones the generator and the walking code already
 * use (`world/footprints.ts`): the terminals and machines, the furniture,
 * the floor props, the heroes that stand on the floor and the console
 * room's fittings, plus the two wall props that stand on the floor against
 * their wall (`WALL_STANDING`).
 */

import { interiorFootprint } from "../world/consoleRoom";
import {
  HERO_FOOTING,
  decorFootprint,
  footprintOf,
  heroFootprint,
  propFootprint,
} from "../world/footprints";
import type {
  Box,
  DecorKind,
  FloorPropKind,
  HeroKind,
  RoomSpec,
} from "../world/types";
import { CELL } from "../world/units";
import { frameAt } from "./kit";
import { WALL_STANDING } from "./models/props/wall";

/** Texels a metre. */
export const SHADOW_TEXELS = 8;

/**
 * The round floor props, per variant: the ones that stand on a round
 * base. The barrel cluster (variant 1) counts, its three barrels read as
 * one round patch.
 */
const ROUND_PROPS: Partial<Record<FloorPropKind, readonly boolean[]>> = {
  barrel: [true, true],
  "traffic-cone": [true, true],
  stool: [true, false],
  planter: [true, false],
  "cable-coil": [true, true],
  "potted-tree": [true, true],
};

/** The round furniture. */
const ROUND_DECOR: ReadonlySet<DecorKind> = new Set([
  "round-table",
  "specimen-tank",
]);

/** The round heroes. */
const ROUND_HEROES: ReadonlySet<HeroKind> = new Set([
  "turret",
  "sleep-ring",
  "moon-rocket",
]);

/** One patch: centred on `(x, z)`, `hx` by `hz` half-extents, in metres. */
export interface Caster {
  x: number;
  z: number;
  hx: number;
  hz: number;
  /** A disc (an ellipse in the footprint) instead of a rectangle. */
  round: boolean;
}

/**
 * How dark a patch's middle gets, 0 to 1, by the footprint's size (the
 * square root of its area) alone: full for small and medium props, up to
 * `DARK_FULL` across, then lighter as the footprint grows, down to
 * `DARK_LEAST`, so a big cabinet or machine stands on a soft shade rather
 * than a painted pad.
 */
export function casterDarkness(c: Caster): number {
  const size = 2 * Math.sqrt(c.hx * c.hz);
  return Math.max(DARK_LEAST, Math.min(1, 1 - 0.35 * (size - DARK_FULL)));
}

/** The footprint size, in metres, up to which a patch is fully dark. */
const DARK_FULL = 0.8;

/** The lightest a large footprint's patch gets. */
const DARK_LEAST = 0.55;

/**
 * How far a patch's soft edge runs past the footprint grown by `MARGIN`,
 * in metres; it starts `EDGE_IN` of that inside it. A little wider as the
 * footprint grows, never wider than `SOFT_MAX`.
 */
export function casterSoftness(c: Caster): number {
  const size = 2 * Math.sqrt(c.hx * c.hz);
  return Math.min(SOFT_MAX, 0.2 + 0.1 * size);
}

/** The widest soft edge, in metres. */
const SOFT_MAX = 0.5;

/**
 * How far past its footprint a patch keeps its full darkness, in metres,
 * so the shadow shows round the prop's foot on the dark floor.
 */
export const MARGIN = 0.05;

/** How far inside the footprint the soft edge starts, as a share of its width. */
const EDGE_IN = 0.5;

/** A box's patch. */
function fromBox(b: Box, round: boolean): Caster {
  return {
    x: (b.x0 + b.x1) / 2,
    z: (b.z0 + b.z1) / 2,
    hx: (b.x1 - b.x0) / 2,
    hz: (b.z1 - b.z0) / 2,
    round,
  };
}

/** Every patch of the room, in world metres. */
export function shadowCasters(room: RoomSpec): Caster[] {
  const out: Caster[] = [];
  for (const f of room.fixtures) {
    const box = footprintOf(f);
    if (box !== null) out.push(fromBox(box, false));
  }
  for (const d of room.decor) {
    const box = decorFootprint(d);
    if (box !== null) out.push(fromBox(box, ROUND_DECOR.has(d.kind)));
  }
  for (const p of room.props) {
    const box = propFootprint(p);
    if (box !== null) {
      const round = ROUND_PROPS[p.kind as FloorPropKind]?.[p.variant];
      out.push(fromBox(box, round === true));
      continue;
    }
    const wall =
      p.anchor === "wall"
        ? WALL_STANDING[p.kind as keyof typeof WALL_STANDING]?.[p.variant]
        : undefined;
    if (wall === undefined) continue;
    // Along the wall and out from it into the room, as the prop's frame
    // turns them.
    const { along, inward } = frameAt([0, 0, 0], p.turn);
    const mid = (wall.a0 + wall.a1) / 2;
    const half = (wall.a1 - wall.a0) / 2;
    out.push({
      x: p.x * CELL + along[0] * mid + inward[0] * (wall.depth / 2),
      z: p.y * CELL + along[2] * mid + inward[2] * (wall.depth / 2),
      hx: Math.abs(along[0]) * half + Math.abs(inward[0]) * (wall.depth / 2),
      hz: Math.abs(along[2]) * half + Math.abs(inward[2]) * (wall.depth / 2),
      round: false,
    });
  }
  for (const h of room.heroes) {
    if (HERO_FOOTING[h.kind] === "flush") continue;
    out.push(fromBox(heroFootprint(h), ROUND_HEROES.has(h.kind)));
  }
  for (const p of room.interior ?? []) {
    const box = interiorFootprint(p);
    if (box !== null) out.push(fromBox(box, false));
  }
  return out;
}

/**
 * How far `(x, z)` lies outside a patch's footprint, in metres, negative
 * inside it: to the rectangle's nearest edge, or, for a disc, along the
 * ellipse's radius through the point.
 */
function outside(c: Caster, x: number, z: number): number {
  const dx = Math.abs(x - c.x);
  const dz = Math.abs(z - c.z);
  if (c.round) {
    const r = Math.hypot(dx / c.hx, dz / c.hz);
    return (r - 1) * Math.min(c.hx, c.hz);
  }
  const ox = dx - c.hx;
  const oz = dz - c.hz;
  if (ox > 0 && oz > 0) return Math.hypot(ox, oz);
  return Math.max(ox, oz);
}

/** The room's contact shadow: `width` by `depth` texels, 255 the darkest. */
export function contactShadows(room: RoomSpec): {
  width: number;
  depth: number;
  texels: Uint8Array;
} {
  const width = room.width * CELL * SHADOW_TEXELS;
  const depth = room.depth * CELL * SHADOW_TEXELS;
  const texels = new Uint8Array(width * depth);
  for (const foot of shadowCasters(room)) {
    const c = { ...foot, hx: foot.hx + MARGIN, hz: foot.hz + MARGIN };
    const soft = casterSoftness(foot);
    const inset = Math.min(soft * EDGE_IN, c.hx, c.hz);
    const dark = casterDarkness(foot);
    const tx0 = Math.max(0, Math.floor((c.x - c.hx - soft) * SHADOW_TEXELS));
    const tx1 = Math.min(
      width - 1,
      Math.ceil((c.x + c.hx + soft) * SHADOW_TEXELS),
    );
    const tz0 = Math.max(0, Math.floor((c.z - c.hz - soft) * SHADOW_TEXELS));
    const tz1 = Math.min(
      depth - 1,
      Math.ceil((c.z + c.hz + soft) * SHADOW_TEXELS),
    );
    for (let tz = tz0; tz <= tz1; tz++) {
      const z = (tz + 0.5) / SHADOW_TEXELS;
      for (let tx = tx0; tx <= tx1; tx++) {
        const x = (tx + 0.5) / SHADOW_TEXELS;
        // 1 from `inset` inside the grown footprint in, falling smoothly
        // to 0 at `soft` outside it.
        const t = Math.min(
          1,
          Math.max(0, (soft - outside(c, x, z)) / (soft + inset)),
        );
        const v = Math.round(255 * dark * t * t * (3 - 2 * t));
        const i = tz * width + tx;
        if (v > (texels[i] ?? 0)) texels[i] = v;
      }
    }
  }
  return { width, depth, texels };
}
