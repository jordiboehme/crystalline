/**
 * The retro desk curios' recipes (2.6b): the pocket console standing on its
 * foot, the home-computer tape drive, the portable tape player with its
 * headphones, the video tape (in its sleeve or lying bare beside it) and
 * the beige laptop. Each kind starts as the blockout (`curioBlockout`);
 * the batch's model task replaces them, keeping its colours and helpers in
 * this file and importing only `common.ts`.
 */

import type { CurioKind } from "../../../world/types";
import { curioBlockout, type CurioRecipe } from "./common";

/** The retro kinds' recipes, one per kind. */
export const RETRO_RECIPES = {
  "pocket-console": curioBlockout,
  "tape-drive": curioBlockout,
  "tape-player": curioBlockout,
  "video-tape": curioBlockout,
  "beige-laptop": curioBlockout,
} satisfies Record<
  Extract<
    CurioKind,
    | "pocket-console"
    | "tape-drive"
    | "tape-player"
    | "video-tape"
    | "beige-laptop"
  >,
  CurioRecipe
>;
