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
import type { FloorPropKind } from "../../../world/types";
import { blockout, type PropRecipe } from "./common";

/** A floor blockout's height, in metres. */
const BLOCKOUT_HEIGHT = 0.6;

/**
 * A floor prop's blockout: a box filling the footprint of the variant it
 * is built as, read from `FOOTPRINTS.prop` for `kind`. Throws on a variant
 * the kind does not have.
 */
function floorBlockout(kind: FloorPropKind): PropRecipe {
  return ({ k, s, variant }) => {
    const size = FOOTPRINTS.prop[kind][variant];
    if (!size)
      throw new Error(
        `floor blockout: ${kind} has no variant ${String(variant)}`,
      );
    const { width, depth } = size;
    blockout(
      k,
      s,
      [-width / 2, width / 2],
      [-depth / 2, depth / 2],
      [0, BLOCKOUT_HEIGHT],
    );
  };
}

/** The recipe of every floor prop kind, the condition extras included. */
export const FLOOR_RECIPES = {
  crate: floorBlockout("crate"),
  barrel: floorBlockout("barrel"),
  trolley: floorBlockout("trolley"),
  stool: floorBlockout("stool"),
  "filing-cabinet": floorBlockout("filing-cabinet"),
  "storage-shelf": floorBlockout("storage-shelf"),
  planter: floorBlockout("planter"),
  bench: floorBlockout("bench"),
  "specimen-shelf": floorBlockout("specimen-shelf"),
  "fume-cabinet": floorBlockout("fume-cabinet"),
  "traffic-cone": floorBlockout("traffic-cone"),
  ladder: floorBlockout("ladder"),
  "tool-cart": floorBlockout("tool-cart"),
  "toppled-crate": floorBlockout("toppled-crate"),
  "debris-pile": floorBlockout("debris-pile"),
  "cable-coil": floorBlockout("cable-coil"),
} satisfies Record<FloorPropKind, PropRecipe>;
