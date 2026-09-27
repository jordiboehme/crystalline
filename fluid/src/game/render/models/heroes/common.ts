/**
 * What every hero recipe shares: the shape of a recipe, each kind's blink
 * bank, the sizes a recipe reads, the blockout every kind starts as, and
 * the small pieces more than one batch uses (the status light colours and
 * `cornerPosts`, a bench's or a rack's legs).
 *
 * A hero mesh is built like a prop's: once per kind, variant and look, at
 * the origin in `frameAt([0, 0, 0], 0)`, and drawn instanced, turned by
 * the instance's quarter turn and moved to its anchor on the GPU. Its
 * envelope is the hero's own footprint (`heroFootprint` in
 * `world/footprints.ts`) and its own top (`HERO_CATALOGUE[kind].top`), not
 * a prop band. A `free` hero is built centred on the origin; a
 * wall-anchored one (flush or backed) from the wall point outward along
 * `d`, its back on the wall at `d = 0`. `heroHalf` gives both in the
 * recipe's local terms, so no recipe repeats a catalogue number. A
 * hovering hero (`heroLift`, C4) is built at its hover height: the
 * instance still stands at `y` 0, and the mesh's lowest vertex is the
 * lift.
 *
 * A hero's lights: a steady one uses `s.signal(tint)` (or `s.glow(tint)`
 * for a screen that carries the panel texture), a blinking one
 * `s.blink(tint, group)`, whose bank is the kind's `HERO_BANK` entry (H11).
 * The instance slot carries the bank, so every group of a kind's lights
 * pulses with that bank. A kind whose bank is `steady` has no blink parts,
 * and every other kind has at least one (the hero test holds both).
 *
 * Kept apart from `index.ts` so the recipe files can import it without an
 * import cycle through the dispatcher.
 */

import { FOOTPRINTS, HERO_FOOTING, heroLift } from "../../../world/footprints";
import { HERO_CATALOGUE } from "../../../world/heroes";
import type { HeroKind } from "../../../world/types";
import type { BlinkBank } from "../../blink";
import type { Surface } from "../../geometry";
import type { Kit } from "../../kit";
import type { Look, Rgb } from "../../looks";
import type { KitAt, Surfaces } from "../common";

/** Bare aluminium: frames, grilles and trims, shared by every batch. */
export const ALUMINIUM: Rgb = [0.55, 0.55, 0.57];

/**
 * A status light that says all is well, shared by every hero that shows one.
 */
export const STATUS_GREEN: Rgb = [0.25, 1.0, 0.35];

/** A status light that says wait, shared by every hero that shows one. */
export const STATUS_AMBER: Rgb = [1.0, 0.6, 0.12];

/**
 * Square posts standing at every pairing of `as` and `ds` (the outer loop
 * over `as`, the inner over `ds`), each `2 * half` on a side, from `h0` up
 * to `h1`: a bench's or a rack's legs.
 */
export function cornerPosts(
  k: Kit,
  as: readonly number[],
  ds: readonly number[],
  half: number,
  h0: number,
  h1: number,
  s: Surface,
): void {
  for (const a of as)
    for (const d of ds)
      k.box(a - half, a + half, d - half, d + half, h0, h1, s);
}

/**
 * One hero kind's recipe: builds `variant` of `kind` with `k` in
 * `frameAt([0, 0, 0], 0)`. Pure in kind, variant, look and `movers`.
 *
 * `movers` says where the kind's moving parts go (only the police box has
 * any, its two door leaves): true builds them in place, closed, as part of
 * the one mesh, which is what the model checks look at; false leaves them
 * out, because `buildHeroMovers` (`heroes/index.ts`) builds them as movers
 * of their own and the instanced mesh must not draw them a second time.
 */
export type HeroRecipe = (r: {
  k: Kit;
  kitAt: KitAt;
  s: Surfaces;
  look: Look;
  variant: number;
  kind: HeroKind;
  movers: boolean;
}) => void;

/**
 * Each hero kind's blink bank (the tables in the 2.6a plan's Task 3 and the
 * 2.6c plan's C13).
 */
export const HERO_BANK = {
  turret: "breathe",
  "black-slab": "steady",
  "eye-panel": "breathe",
  "photo-console": "status",
  "laser-desk": "breathe",
  "mess-table": "steady",
  "helper-robot": "swap",
  "sleep-ring": "chase",
  "dome-planters": "breathe",
  "core-wall": "twinkle",
  "gun-rack": "breathe",
  "gun-bench": "breathe",
  "tube-bench": "chase",
  "field-pack": "chase",
  "arcade-cabinet": "swap",
  "recruit-cabinet": "swap",
  "stone-hand": "steady",
  "question-block": "breathe",
  "mech-head": "breathe",
  "red-bike": "breathe",
  hoverboard: "steady",
  "flying-cloud": "steady",
  "spider-tank": "breathe",
  "garden-robot": "breathe",
  "moon-rocket": "steady",
  "thunder-hammer": "steady",
  "police-box": "breathe",
  "slab-walker": "steady",
} as const satisfies Record<HeroKind, BlinkBank>;

/**
 * A hero's half width and its depth range in the recipe's local terms:
 * `a` from `-hw` to `hw`; `d` from `d0` to `d1`, which is `-depth / 2` to
 * `depth / 2` for a free footing and `0` to `depth` for a wall-anchored
 * one; its `top`, and its `lift` (`heroLift`, C4: the height its
 * underside hovers at, 0 for a kind that stands on the floor). Read the
 * sizes from here, never repeat them in a recipe. Throws on a variant the
 * kind does not have.
 */
export function heroHalf(
  kind: HeroKind,
  variant: number,
): { hw: number; d0: number; d1: number; top: number; lift: number } {
  const size = FOOTPRINTS.hero[kind][variant];
  if (size === undefined)
    throw new Error(`heroHalf: ${kind} has no variant ${String(variant)}`);
  const free = HERO_FOOTING[kind] === "free";
  return {
    hw: size.width / 2,
    d0: free ? -size.depth / 2 : 0,
    d1: free ? size.depth / 2 : size.depth,
    top: HERO_CATALOGUE[kind].top,
    lift: heroLift(kind),
  };
}

/** The bevel of a hero blockout's box, in metres. */
const BLOCKOUT_BEVEL = 0.02;

/** A blinking blockout's cap: its side, in metres. */
const CAP_SIDE = 0.1;

/**
 * A blinking blockout's cap: its height, in metres; the box stops this far
 * under the top.
 */
const CAP_HEIGHT = 0.02;

/**
 * The blockout cap's colour: a plain warm white, a stand-in light that
 * belongs to no look and no finished model.
 */
const CAP_TINT: Rgb = [1.0, 0.92, 0.8];

/**
 * The blockout every hero starts as (like `blockout` for props): one
 * bevelled box over its whole footprint from its lift (`heroHalf`'s
 * `lift`, 0 for a kind on the floor) up to its top, less 0.02 m for a
 * kind whose bank blinks, which gets a 0.1 m blink-group-0 cap sitting on
 * the box's top in the middle, so every check (glow contact, the bank rule)
 * runs on it from the start. The cap is `CAP_SIDE` square and
 * `CAP_HEIGHT` thick, so it ends exactly at the top.
 */
export const heroBlockout: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw, d0, d1, top, lift } = heroHalf(kind, variant);
  const blinks = HERO_BANK[kind] !== "steady";
  const h1 = blinks ? top - CAP_HEIGHT : top;
  k.bevelBox(-hw, hw, d0, d1, lift, h1, BLOCKOUT_BEVEL, s.body);
  if (!blinks) return;
  const dm = (d0 + d1) / 2;
  const c = CAP_SIDE / 2;
  k.box(-c, c, dm - c, dm + c, h1, top, s.blink(CAP_TINT, 0));
};
