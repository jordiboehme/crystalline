/**
 * What every hand-built room fitting's recipe shares (2.6e C2, C4, M3
 * C24): the shape of a recipe, each kind's blink bank, the sizes a recipe
 * reads and the console room's fixed colours, which the airlock's shell
 * shares.
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
 * breathe softly (the `soft` bank, 2.6e C26: never far under their peak,
 * which stays under the bloom threshold), the console's small lights
 * twinkle out of step, the airlock's two beacons (M3 C24) take turns on
 * the `swap` bank (one in groups 0 to 3, the other in 4 to 7, each half
 * `SWAP_TICS` long), and the inner doors, the scanner, the outer hatch,
 * the iris light and the suit lockers have no blinking light.
 */
export const INTERIOR_BANK = {
  "roundel-wall": "soft",
  "inner-doors": "steady",
  scanner: "steady",
  console: "twinkle",
  "outer-hatch": "steady",
  beacon: "swap",
  "iris-light": "steady",
  "suit-locker": "steady",
} as const satisfies Record<InteriorKind, BlinkBank>;

/**
 * The console room's walls and the fittings' white (C4, C25). Above 1 on
 * purpose: the look's panel texture (about 0.87 on average) and the fixed
 * shading by face direction (0.82 to 0.96) would leave a plain white at a
 * light grey. At this tint a wall comes out near 0.84 before the tone
 * map, the brightest neutral surface in the room, and even a face turned
 * up to the light stays under the aperture grid's bloom threshold (0.9),
 * so nothing white blooms.
 */
export const CONSOLE_WALL: Rgb = [1.06, 1.06, 1.04];

/**
 * A roundel's shaded inner face (C4, C5, C25): a shade darker than the
 * wall, so the discs read as pale recesses in it, not as dark holes.
 */
export const ROUNDEL_FACE: Rgb = shade(CONSOLE_WALL, 0.9);

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
 * `depth / 2` for a free or overhead piece (the console, the iris light)
 * and `0` to `depth` for a flush or backed one; and its `top`. Read the sizes from here, never repeat them in a recipe.
 */
export function interiorHalf(kind: InteriorKind): {
  hw: number;
  d0: number;
  d1: number;
  top: number;
} {
  const entry = INTERIOR_CATALOGUE[kind];
  const free = entry.footing === "free" || entry.footing === "overhead";
  return {
    hw: entry.width / 2,
    d0: free ? -entry.depth / 2 : 0,
    d1: free ? entry.depth / 2 : entry.depth,
    top: entry.top,
  };
}
