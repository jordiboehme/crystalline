/**
 * The floor props' recipes: everything that stands on the floor and
 * collides with the player, the condition extras included.
 *
 * A floor prop stays inside its variant's footprint (`FOOTPRINTS.prop` in
 * `world/footprints.ts`), as it stands at turn 0: `width` along `a`, `depth`
 * along `d`, centred on the anchor, and below `FLOOR_TOP`. For now every
 * kind is a blockout; the real models replace them here, kind by kind.
 */

import { FOOTPRINTS } from "../../../world/footprints";
import type { FloorPropKind, PropKind } from "../../../world/types";
import { blockout, type PropRecipe } from "./common";

/** A floor blockout's height, in metres. */
const BLOCKOUT_HEIGHT = 0.6;

/**
 * The size of a floor prop's variant, as it stands at turn 0. Throws on a
 * kind that is not a floor prop or a variant it does not have.
 */
export function floorSize(
  kind: PropKind,
  variant: number,
): { width: number; depth: number } {
  const sizes = (
    FOOTPRINTS.prop as Partial<
      Record<PropKind, readonly { width: number; depth: number }[]>
    >
  )[kind];
  const size = sizes?.[variant];
  if (!size)
    throw new Error(`floorSize: ${kind} has no variant ${String(variant)}`);
  return size;
}

/** A floor prop's blockout: a box filling its variant's footprint. */
const floorBlockout: PropRecipe = ({ k, s, kind, variant }) => {
  const { width, depth } = floorSize(kind, variant);
  blockout(
    k,
    s,
    [-width / 2, width / 2],
    [-depth / 2, depth / 2],
    [0, BLOCKOUT_HEIGHT],
  );
};

/** The recipe of every floor prop kind, the condition extras included. */
export const FLOOR_RECIPES = {
  crate: floorBlockout,
  barrel: floorBlockout,
  trolley: floorBlockout,
  stool: floorBlockout,
  "filing-cabinet": floorBlockout,
  "storage-shelf": floorBlockout,
  planter: floorBlockout,
  bench: floorBlockout,
  "specimen-shelf": floorBlockout,
  "fume-cabinet": floorBlockout,
  "traffic-cone": floorBlockout,
  ladder: floorBlockout,
  "tool-cart": floorBlockout,
  "toppled-crate": floorBlockout,
  "debris-pile": floorBlockout,
  "cable-coil": floorBlockout,
} satisfies Record<FloorPropKind, PropRecipe>;
