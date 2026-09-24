/**
 * What hangs flat on a wall: posters, the placard and a machine's tag
 * strip.
 *
 * - A poster is the observations of one category, a printed sheet in a
 *   thin bevelled frame, pinned at each corner.
 * - The placard at the entrance is the frontmatter, a plaque standing off
 *   the wall on two brackets.
 * - A tag strip runs along the wall above each machine in the tag's
 *   colour (milestone 1's strip), with the tag's name on a label below
 *   the ceiling.
 */

import type { Fixture } from "../../world/types";
import { FLAG } from "../geometry";
import { frameForSlot, type Kit } from "../kit";
import { ASPECT } from "../layers";
import type { Rgb } from "../looks";
import {
  HEADROOM,
  label,
  surfaces,
  textPanel,
  type KitAt,
  type ModelContext,
} from "./common";

type Poster = Extract<Fixture, { kind: "poster" }>;
type Placard = Extract<Fixture, { kind: "placard" }>;

/** A poster sheet's half width and bottom edge, and its frame's border. */
const SHEET_HALF = 0.5;
const SHEET_BOTTOM = 1.2;
const POSTER_BORDER = 0.05;
const POSTER_D = 0.03;

/** Builds a poster against its wall slot. */
export function buildPoster(
  kitAt: KitAt,
  fx: Poster,
  index: number,
  ctx: ModelContext,
): void {
  const k = kitAt(frameForSlot(fx.slot));
  const s = surfaces(ctx.look);
  const top = SHEET_BOTTOM + (2 * SHEET_HALF) / ASPECT.placard;
  const b = POSTER_BORDER;
  k.bevelBox(
    -SHEET_HALF - b,
    SHEET_HALF + b,
    0,
    POSTER_D,
    SHEET_BOTTOM - b,
    top + b,
    0.01,
    s.metal,
  );
  textPanel(
    k,
    ctx,
    `poster:${index}`,
    -SHEET_HALF,
    SHEET_HALF,
    POSTER_D + 0.001,
    SHEET_BOTTOM,
    top,
    {
      tint: ctx.look.palette.panel,
      flag: FLAG.lit,
    },
  );
  // A pin in each corner of the sheet.
  for (const a of [-SHEET_HALF + 0.03, SHEET_HALF - 0.05]) {
    for (const h of [SHEET_BOTTOM + 0.03, top - 0.05]) {
      k.bevelBox(
        a,
        a + 0.02,
        POSTER_D,
        POSTER_D + 0.012,
        h,
        h + 0.02,
        0.004,
        s.dark,
      );
    }
  }
}

/** The placard's plaque: half width, bottom edge, depth range. */
const PLAQUE_HALF = 0.5;
const PLAQUE_BOTTOM = 1.3;
const PLAQUE_D0 = 0.08;
const PLAQUE_D1 = 0.11;
const PLAQUE_BORDER = 0.04;

/** Builds the placard against its wall slot. */
export function buildPlacard(
  kitAt: KitAt,
  fx: Placard,
  ctx: ModelContext,
): void {
  const k = kitAt(frameForSlot(fx.slot));
  const s = surfaces(ctx.look);
  const top = PLAQUE_BOTTOM + (2 * PLAQUE_HALF) / ASPECT.placard;
  const b = PLAQUE_BORDER;
  // Two brackets from the wall, then the plaque on them.
  for (const h of [PLAQUE_BOTTOM + 0.05, top - 0.13]) {
    k.bevelBox(-0.3, 0.3, 0, PLAQUE_D0, h, h + 0.08, 0.01, s.dark);
  }
  k.bevelBox(
    -PLAQUE_HALF - b,
    PLAQUE_HALF + b,
    PLAQUE_D0,
    PLAQUE_D1,
    PLAQUE_BOTTOM - b,
    top + b,
    0.012,
    s.metal,
  );
  textPanel(
    k,
    ctx,
    "placard",
    -PLAQUE_HALF,
    PLAQUE_HALF,
    PLAQUE_D1 + 0.001,
    PLAQUE_BOTTOM,
    top,
    {
      tint: ctx.look.palette.panel,
      flag: FLAG.lit,
    },
  );
}

/** The tag strip: half width, bottom, height, depth; the label above it. */
const STRIP_HALF = 0.95;
const STRIP_H0 = 2.5;
const STRIP_H1 = 2.58;
const STRIP_D = 0.03;
const TAG_LABEL_HALF = 0.9;

/**
 * The tag strip above a machine, in the tag's colour, and the tag's name
 * on a label over it, squeezed under a low ceiling. `key` is the machine's
 * `tag:<index>`.
 */
export function tagStrip(
  k: Kit,
  ctx: ModelContext,
  key: string,
  hue: Rgb,
): void {
  const s = surfaces(ctx.look);
  k.box(-STRIP_HALF, STRIP_HALF, 0, STRIP_D, STRIP_H0, STRIP_H1, s.glow(hue));
  const bottom = Math.min(STRIP_H1 + 0.03, ctx.ceiling - HEADROOM - 0.1);
  label(k, ctx, key, -TAG_LABEL_HALF, TAG_LABEL_HALF, bottom, STRIP_D, hue);
}
