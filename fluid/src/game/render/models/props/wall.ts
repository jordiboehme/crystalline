/**
 * The wall props' recipes: everything that hangs or stands flush against a
 * wall edge, and the two wall runs.
 *
 * A wall prop stays within `WALL_REACH` of its anchor along the wall,
 * within `FLUSH_DEPTH` of the wall and between the floor and `WALL_TOP`; a
 * run segment spans the whole edge (`RUN_REACH`) in `RUN_BAND`, above
 * every wall prop. For now every kind is a blockout; the real models
 * replace them here, kind by kind.
 */

import type { WallPropKind } from "../../../world/types";
import { blockout, type PropRecipe } from "./common";

/** A wall prop's blockout: a box on the wall at chest height. */
const wallBlockout: PropRecipe = ({ k, s }) => {
  blockout(k, s, [-0.3, 0.3], [0, 0.15], [1.0, 1.5]);
};

/** A wall run segment's blockout: a bar across the whole edge, above the props. */
const runBlockout: PropRecipe = ({ k, s }) => {
  blockout(k, s, [-1.0, 1.0], [0.05, 0.25], [2.5, 2.7]);
};

/** The recipe of every wall prop kind, runs included. */
export const WALL_RECIPES = {
  "locker-bank": wallBlockout,
  extinguisher: wallBlockout,
  "first-aid": wallBlockout,
  intercom: wallBlockout,
  "keycard-reader": wallBlockout,
  "vent-grille": wallBlockout,
  "sign-plate": wallBlockout,
  "breaker-box": wallBlockout,
  "wall-monitor": wallBlockout,
  "padded-panel": wallBlockout,
  "light-strip": wallBlockout,
  "cable-tray": runBlockout,
  "pipe-bundle": runBlockout,
} satisfies Record<WallPropKind, PropRecipe>;
