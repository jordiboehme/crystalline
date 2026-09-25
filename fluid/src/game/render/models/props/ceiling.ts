/**
 * The ceiling props' recipes: everything that hangs from the ceiling, out
 * of the player's way, and the two ceiling runs.
 *
 * A ceiling prop hangs from its anchor at the room's ceiling, so its `h`
 * runs from `-CEILING_DROP` to `-HEADROOM`; it stands `CEILING_SETBACK` to
 * `CEILING_OUT` out from its wall and within `WALL_REACH` of its anchor
 * along it, or `RUN_REACH` for a run segment. For now every kind is a
 * blockout; the real models replace them here, kind by kind.
 */

import type { CeilingPropKind } from "../../../world/types";
import { blockout, type PropRecipe } from "./common";

/** A ceiling prop's blockout: a box hanging under the ceiling, off the wall. */
const ceilingBlockout: PropRecipe = ({ k, s }) => {
  blockout(k, s, [-0.3, 0.3], [0.4, 0.8], [-0.4, -0.1]);
};

/** A ceiling run segment's blockout: the same box across the whole edge. */
const runBlockout: PropRecipe = ({ k, s }) => {
  blockout(k, s, [-1.0, 1.0], [0.4, 0.8], [-0.4, -0.1]);
};

/** The recipe of every ceiling prop kind, runs included. */
export const CEILING_RECIPES = {
  duct: runBlockout,
  "ceiling-tray": runBlockout,
  "cable-loop": ceilingBlockout,
  beacon: ceilingBlockout,
  "loose-cable": ceilingBlockout,
} satisfies Record<CeilingPropKind, PropRecipe>;
