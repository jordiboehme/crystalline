/**
 * What every curio recipe shares (2.6b): the shape of a recipe, each
 * kind's blink bank, the sizes a recipe reads and the blockout every kind
 * starts as.
 *
 * A curio mesh is built like a hero's: once per kind, variant and look, at
 * the origin in `frameAt([0, 0, 0], 0)`, and drawn instanced, turned by the
 * instance's quarter turn and moved to its anchor on the GPU, where the
 * anchor's height is the surface it stands on (`Curio.h`). Every curio is
 * built centred on the origin, its base at `h` 0 and its front towards
 * `+d`, inside its catalogue size (`CURIO_CATALOGUE`); `curioHalf` gives
 * that box in the recipe's local terms, so no recipe repeats a catalogue
 * number.
 *
 * A curio's lights: a steady one uses `s.signal(tint)` (or `s.glow(tint)`
 * for a screen), a blinking one `s.blink(tint, group)`, whose bank is the
 * kind's `CURIO_BANK` entry (C16). The instance slot carries the bank, so
 * every group of a kind's lights pulses with that bank. A kind whose bank
 * is `steady` has no blink parts, and every other kind has at least one in
 * every variant (the curio test holds both).
 *
 * Kept apart from `index.ts` so the batch files can import it without an
 * import cycle through the dispatcher. The batch files import only this
 * module, never each other.
 */

import { CURIO_CATALOGUE } from "../../../world/curios";
import type { CurioKind } from "../../../world/types";
import type { BlinkBank } from "../../blink";
import type { Kit } from "../../kit";
import type { Look, Rgb } from "../../looks";
import type { KitAt, Surfaces } from "../common";

/** One curio kind's recipe: builds `variant` of `kind` with `k` in `frameAt([0, 0, 0], 0)`, centred on the origin. Pure in kind, variant and look. */
export type CurioRecipe = (r: {
  k: Kit;
  kitAt: KitAt;
  s: Surfaces;
  look: Look;
  variant: number;
  kind: CurioKind;
}) => void;

/**
 * Each curio kind's blink bank (C16): the sword's blade and the pistol's
 * chamber breathe, the console's screen swaps between two pictures, the
 * meter's wing lights chase, the trap's lights strobe, and every other
 * kind is steady.
 */
export const CURIO_BANK = {
  "light-sword": "breathe",
  "green-pistol": "breathe",
  "pink-gadget": "steady",
  "wing-meter": "chase",
  "pocket-console": "swap",
  "tape-drive": "steady",
  "tape-player": "steady",
  "video-tape": "steady",
  "beige-laptop": "steady",
  "star-ball": "steady",
  "catch-ball": "steady",
  "trap-box": "status",
  "fuel-case": "steady",
} as const satisfies Record<CurioKind, BlinkBank>;

/**
 * A curio's half width, half depth and top in the recipe's local terms:
 * its box runs `a` from `-hw` to `hw`, `d` from `-hd` to `hd` and `h` from
 * 0 to `top`, centred on the origin. Read the sizes from here, never
 * repeat them in a recipe. Throws on a variant the kind does not have.
 */
export function curioHalf(
  kind: CurioKind,
  variant: number,
): { hw: number; hd: number; top: number } {
  const size = CURIO_CATALOGUE[kind].sizes[variant];
  if (size === undefined)
    throw new Error(`curioHalf: ${kind} has no variant ${String(variant)}`);
  return { hw: size.width / 2, hd: size.depth / 2, top: size.top };
}

/** The bevel of a curio blockout's box, in metres. */
const BLOCKOUT_BEVEL = 0.005;

/** A blinking blockout's cap: its side, in metres. */
const CAP_SIDE = 0.03;

/** A blinking blockout's cap: its height, in metres; the box stops this far under the top. */
const CAP_HEIGHT = 0.01;

/**
 * The blockout cap's colour: a plain warm white, a stand-in light that
 * belongs to no look and no finished model.
 */
const CAP_TINT: Rgb = [1.0, 0.92, 0.8];

/**
 * The blockout every curio starts as (like `heroBlockout`): one bevelled
 * box over its whole size up to its top, less `CAP_HEIGHT` for a kind
 * whose bank blinks, which gets a blink-group-0 cap sitting on the box's
 * top in the middle, so every check (glow contact, the bank rule) runs on
 * it from the start. The cap is `CAP_SIDE` square and `CAP_HEIGHT` thick,
 * so it ends exactly at the top.
 */
export const curioBlockout: CurioRecipe = ({ k, s, variant, kind }) => {
  const { hw, hd, top } = curioHalf(kind, variant);
  const blinks = CURIO_BANK[kind] !== "steady";
  const h1 = blinks ? top - CAP_HEIGHT : top;
  k.bevelBox(-hw, hw, -hd, hd, 0, h1, BLOCKOUT_BEVEL, s.body);
  if (!blinks) return;
  const c = CAP_SIDE / 2;
  k.box(-c, c, -c, c, h1, top, s.blink(CAP_TINT, 0));
};
