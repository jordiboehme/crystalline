/**
 * What every console room fitting's recipe shares (2.6e C2, C4): the shape
 * of a recipe, each kind's blink bank, the sizes a recipe reads, the
 * blockout every kind starts as, and the room's fixed colours.
 *
 * A fitting's mesh is built like a hero's: once per kind, variant and look,
 * at the origin in `frameAt([0, 0, 0], 0)`, and drawn instanced, turned by
 * the instance's quarter turn and moved to its anchor on the GPU. A flush
 * piece is built from its wall point outward along `d`, its back on the
 * wall at `d = 0`; the free console is built centred on the origin.
 * `interiorHalf` gives both in the recipe's local terms, so no recipe
 * repeats a catalogue number (`INTERIOR_CATALOGUE` in
 * `world/consoleRoom.ts`).
 *
 * The console room's colours hold in every look (C4): its walls, roundel
 * faces, floor and ceiling are fixed tints, like the police box's blue.
 * The look still sets the light, the edge lines, the bloom and the
 * palette of its own era. `CONSOLE_SHELL` is the room shell's share of
 * them, which `buildRoomMesh` draws a room with `interior` in.
 *
 * A fitting's lights: a steady one uses `s.signal(tint)`, a blinking one
 * `s.blink(tint, group)`, whose bank is the kind's `INTERIOR_BANK` entry;
 * the instance slot carries the bank. A kind whose bank is `steady` has no
 * blink parts in any variant.
 *
 * Kept apart from `index.ts` so the recipe files can import it without an
 * import cycle through the dispatcher. It holds plain tints, not surfaces,
 * so it reads nothing of `geometry.ts` at load time.
 */

import { INTERIOR_CATALOGUE } from "../../../world/consoleRoom";
import type { InteriorKind } from "../../../world/types";
import type { BlinkBank } from "../../blink";
import type { Kit } from "../../kit";
import type { Look, Rgb } from "../../looks";
import { shade, type KitAt, type Surfaces } from "../common";

/**
 * One fitting kind's recipe: builds `variant` of `kind` with `k` in
 * `frameAt([0, 0, 0], 0)`. Pure in kind, variant and look. A fitting's
 * moving part (the console's rotor) is not built here but by
 * `buildInteriorMovers` (`interior/index.ts`), in world space.
 */
export type InteriorRecipe = (r: {
  k: Kit;
  kitAt: KitAt;
  s: Surfaces;
  look: Look;
  variant: number;
  kind: InteriorKind;
}) => void;

/**
 * Each fitting kind's blink bank: the roundel walls' glowing roundels
 * breathe, the console's small lights twinkle out of step, and the inner
 * doors and the scanner have no blinking light.
 */
export const INTERIOR_BANK = {
  "roundel-wall": "breathe",
  "inner-doors": "steady",
  scanner: "steady",
  console: "twinkle",
} as const satisfies Record<InteriorKind, BlinkBank>;

/** The console room's walls and the fittings' white (C4). */
export const CONSOLE_WALL: Rgb = [0.93, 0.93, 0.9];

/** A roundel's shaded inner face (C4, C5). */
export const ROUNDEL_FACE: Rgb = shade(CONSOLE_WALL, 0.82);

/** The console room's pale floor (C4). */
export const CONSOLE_FLOOR: Rgb = [0.78, 0.78, 0.74];

/** The console room's ceiling (C4). */
export const CONSOLE_CEILING_TINT: Rgb = [0.9, 0.9, 0.88];

/**
 * The tints `buildRoomMesh` draws the shell of a room with `interior` in,
 * whatever the look (C4): the walls (covered by the flush pieces), the
 * floor and the ceiling. The layers stay the shell's own.
 */
export const CONSOLE_SHELL: { wall: Rgb; floor: Rgb; ceiling: Rgb } = {
  wall: CONSOLE_WALL,
  floor: CONSOLE_FLOOR,
  ceiling: CONSOLE_CEILING_TINT,
};

/**
 * A fitting's half width and its depth range in the recipe's local terms:
 * `a` from `-hw` to `hw`; `d` from `d0` to `d1`, which is `-depth / 2` to
 * `depth / 2` for the free console and `0` to `depth` for a flush piece;
 * and its `top`. Read the sizes from here, never repeat them in a recipe.
 */
export function interiorHalf(kind: InteriorKind): {
  hw: number;
  d0: number;
  d1: number;
  top: number;
} {
  const entry = INTERIOR_CATALOGUE[kind];
  const free = entry.footing === "free";
  return {
    hw: entry.width / 2,
    d0: free ? -entry.depth / 2 : 0,
    d1: free ? entry.depth / 2 : entry.depth,
    top: entry.top,
  };
}

/** The bevel of a blockout's box, in metres. */
const BLOCKOUT_BEVEL = 0.02;

/** A blinking blockout's cap: its side, in metres (less on a thin piece). */
const CAP_SIDE = 0.1;

/** A blinking blockout's cap: its height, in metres; the box stops this far under the top. */
const CAP_HEIGHT = 0.02;

/**
 * The blockout cap's colour: a plain warm white, a stand-in light that
 * belongs to no finished model.
 */
const CAP_TINT: Rgb = [1.0, 0.92, 0.8];

/**
 * The blockout every fitting starts as (like `heroBlockout`): one bevelled
 * box over its whole footprint from the floor up to its top, less 0.02 m
 * for a kind whose bank blinks, which gets a blink-group-0 cap on the
 * box's top in the middle, so every check (glow contact, the bank rule)
 * runs on it from the start. The cap is `CAP_SIDE` wide and `CAP_HEIGHT`
 * thick and ends exactly at the top; on a piece thinner than `CAP_SIDE` it
 * spans the piece's depth, so it never leaves the envelope.
 */
export const interiorBlockout: InteriorRecipe = ({ k, s, kind }) => {
  const { hw, d0, d1, top } = interiorHalf(kind);
  const blinks = INTERIOR_BANK[kind] !== "steady";
  const h1 = blinks ? top - CAP_HEIGHT : top;
  k.bevelBox(-hw, hw, d0, d1, 0, h1, BLOCKOUT_BEVEL, s.body);
  if (!blinks) return;
  const dm = (d0 + d1) / 2;
  const c = CAP_SIDE / 2;
  const cd = Math.min(c, (d1 - d0) / 2);
  k.box(-c, c, dm - cd, dm + cd, h1, top, s.blink(CAP_TINT, 0));
};
