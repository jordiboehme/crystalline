/**
 * The hovering heroes' recipes: the question block, the hoverboard and the
 * flying cloud. What they share is that they hover (C4): each is built at
 * its lift (`heroHalf`'s `lift`), so its lowest vertex is the height it
 * hovers at, while the instance itself stands at `y` 0. The block hangs
 * over the player's head at 2.3 m, the board level at 0.25 m and the
 * cloud's underside at 0.4 m. Only the block has a light: its four marks
 * breathe; the board and the cloud are unlit, as their originals are.
 *
 * The block's mark (C12) is the one readable thing here: a "?" drawn as a
 * pixel picture, `QUESTION_MARK`, not a glyph of `PIXEL_FONT`, so the
 * font still refuses "?" and the block stays its only use.
 *
 * The numbers each kind is built to are named in a table above its
 * recipe: `BLOCK` for the block, `BOARD` for the hoverboard and `LUMPS`
 * (with `SQUASH`) for the cloud. Round parts use few facets, as in every
 * batch.
 */

import type { HeroKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import { yawed, type Surfaces } from "../common";
import { heroHalf, type HeroRecipe } from "./common";
import { pixelPanel } from "./pixels";

/** The recipe's own frame: the origin, facing north. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/** The block's face gold. */
const BLOCK_GOLD: Rgb = [0.92, 0.65, 0.1];

/** The block's edges: a darker orange-brown. */
const BLOCK_EDGE: Rgb = [0.6, 0.32, 0.05];

/** The block's corner rivets: a dark brown. */
const BLOCK_RIVET: Rgb = [0.3, 0.18, 0.05];

/** The mark's white, which breathes. */
const MARK_WHITE: Rgb = [0.95, 0.95, 0.9];

/** The mark's drop shadow: the rivets' dark brown. */
const MARK_SHADOW: Rgb = [0.3, 0.18, 0.05];

/**
 * The block's mark (C12): a chunky "?" two pixels thick (`#`) with a
 * one-pixel drop shadow to the lower right (`s`), row 0 at the top. It is
 * a picture, not a glyph of `PIXEL_FONT`, so `textRows` still refuses
 * "?": the one approved use of the mark is this block. One of the three
 * approved exceptions to the no-markings rule, with the core wall's
 * nameplate and the police box's sign.
 */
export const QUESTION_MARK: readonly string[] = [
  ".####..",
  "##ss##.",
  ".ss.##s",
  "...##ss",
  "..##ss.",
  "...ss..",
  "..##...",
  "...ss..",
];

/**
 * The block's measures, in metres from its centre, the same on all six
 * faces: the edge-coloured core reaches `face`, a gold plate lies on each
 * face out to `plate` and stops `inset` from the core's edges, so a band
 * of the edge colour frames every face, and a dark rivet `2 * rivet` on a
 * side sits `rivetIn` in from each corner of every plate. The rivets and
 * the marks stand `DECAL_LIFT` proud of the plates, out to the 0.3 half
 * width, the top and the lift exactly. The mark's pixels are `px` square.
 */
export const BLOCK = {
  face: 0.28,
  plate: 0.29,
  inset: 0.03,
  rivet: 0.02,
  rivetIn: 0.05,
  px: 0.05,
} as const;

/**
 * One plated face of the block, in the face's own terms: `u` and `v`
 * across it from its middle, `w` outward from the block's centre. `put`
 * lays a box of those ranges in the kit that owns the face.
 */
type FacePut = (
  u0: number,
  u1: number,
  v0: number,
  v1: number,
  w0: number,
  w1: number,
  s: Surface,
) => void;

/** A face's gold plate and its four rivets, through `put`. */
function platedFace(put: FacePut, gold: Surface, rivet: Surface): void {
  const p = BLOCK.face - BLOCK.inset;
  put(-p, p, -p, p, BLOCK.face, BLOCK.plate, gold);
  const r = p - BLOCK.rivetIn;
  const top = BLOCK.plate + DECAL_LIFT;
  for (const u of [-r, r])
    for (const v of [-r, r])
      put(
        u - BLOCK.rivet,
        u + BLOCK.rivet,
        v - BLOCK.rivet,
        v + BLOCK.rivet,
        BLOCK.plate,
        top,
        rivet,
      );
}

/**
 * The block: an edge-coloured core, a gold plate with four rivets on each
 * of its six faces, and the mark on the four sides, never the top or the
 * bottom. Its bottom rivets rest exactly at the lift and its top ones end
 * exactly at the top.
 */
const questionBlock: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { lift, top } = heroHalf(kind, variant);
  const mid = (lift + top) / 2;
  const edge = s.tinted(BLOCK_EDGE);
  const gold = s.tinted(BLOCK_GOLD);
  const rivet = s.tinted(BLOCK_RIVET);
  const f = BLOCK.face;
  k.box(-f, f, -f, f, mid - f, mid + f, edge);
  // The top and the bottom.
  platedFace(
    (u0, u1, v0, v1, w0, w1, sf) =>
      k.box(u0, u1, v0, v1, mid + w0, mid + w1, sf),
    gold,
    rivet,
  );
  platedFace(
    (u0, u1, v0, v1, w0, w1, sf) =>
      k.box(u0, u1, v0, v1, mid - w1, mid - w0, sf),
    gold,
    rivet,
  );
  const cols = QUESTION_MARK[0]?.length ?? 0;
  const shadow = s.tinted(MARK_SHADOW);
  for (let i = 0; i < 4; i++) {
    const side = kitAt(yawed(ORIGIN, 0, 0, (i * Math.PI) / 2));
    platedFace(
      (u0, u1, v0, v1, w0, w1, sf) =>
        side.box(u0, u1, w0, w1, mid + v0, mid + v1, sf),
      gold,
      rivet,
    );
    pixelPanel(
      side,
      QUESTION_MARK,
      (-cols * BLOCK.px) / 2,
      mid + (QUESTION_MARK.length * BLOCK.px) / 2,
      BLOCK.px,
      BLOCK.plate + DECAL_LIFT,
      (ch) =>
        ch === "#" ? s.blink(MARK_WHITE, 0) : ch === "s" ? shadow : null,
    );
  }
};

/** The board's deck: a hot pink. */
const BOARD_PINK: Rgb = [1.0, 0.25, 0.6];

/** The board's foot grip pads: near black. */
const BOARD_PAD: Rgb = [0.05, 0.05, 0.05];

/** The board's front edge panels: lime green. */
const BOARD_LIME: Rgb = [0.55, 0.9, 0.15];

/** The board's back edge panels: yellow. */
const BOARD_YELLOW: Rgb = [0.98, 0.85, 0.1];

/**
 * The hoverboard's measures, in metres. The deck is `thick` thick and
 * `inset` narrower on each side than the footprint, whose last `inset`
 * the edge panels fill. Along its length it is flat out to `flat` from the
 * centre, then ramps up to a short level `shelf` just before each round
 * end, and each round end rises a last `tipRise` to the top: the kick.
 * Each round end is a fan of `slices` slabs about its half circle's
 * centre, each ending in a chord, so the ends read round from above and
 * every slab's gentle rise is the same. The foot pads sit at `pad` from
 * the middle, `padA` long and `padD` wide in half measures, `padH` thick.
 */
const BOARD = {
  thick: 0.035,
  inset: 0.015,
  flat: 0.24,
  shelf: 0.03,
  tipRise: 0.015,
  slices: 6,
  pad: 0.14,
  padA: 0.075,
  padD: 0.07,
  padH: 0.004,
} as const;

/**
 * The hoverboard: a deck profile along `a` with a kick at each end,
 * extruded across `d` (the middle in one piece, each round end a fan of
 * slabs whose tips follow a half circle), two black foot pads on its flat
 * and edge panels along both sides of the flat, lime towards `+a` and
 * yellow towards `-a`. Level at its lift.
 */
const hoverboard: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, d1, lift, top } = heroHalf(kind, variant);
  const t = BOARD.thick;
  const w = d1 - BOARD.inset;
  // Where the round ends start: the centre of each end's half circle.
  const c = hw - w;
  const ramp = c - BOARD.shelf;
  // The shelf's underside: the kick less the round end's last rise.
  const shelf = top - t - BOARD.tipRise;
  const pink = s.tinted(BOARD_PINK);
  // The underside's height at `x` along the middle piece.
  const under = (x: number): number =>
    lift +
    (shelf - lift) *
      Math.min(1, Math.max(0, Math.abs(x) - BOARD.flat) / (ramp - BOARD.flat));
  const xs = [-c, -ramp, -BOARD.flat, BOARD.flat, ramp, c];
  k.extrude(
    [
      ...xs.map((x): [number, number] => [x, under(x)]),
      ...xs.map((x): [number, number] => [x, under(x) + t]).reverse(),
    ],
    -w,
    w,
    pink,
  );
  // The round ends: slabs fanned about the half circle's centre, each
  // ending in a chord of the circle and rising `tipRise` along its axis.
  // Their inner corners reach back over the shelf, which is level.
  const step = Math.PI / BOARD.slices;
  const apothem = w * Math.cos(step / 2);
  const chord = w * Math.sin(step / 2);
  const tip = top - t;
  for (const sign of [1, -1])
    for (let i = 0; i < BOARD.slices; i++) {
      const theta = -Math.PI / 2 + (i + 0.5) * step;
      kitAt(
        yawed(ORIGIN, sign * c, 0, sign > 0 ? theta : Math.PI + theta),
      ).extrude(
        [
          [0, shelf],
          [apothem, tip],
          [apothem, tip + t],
          [0, shelf + t],
        ],
        -chord,
        chord,
        pink,
      );
    }
  const pad = s.tinted(BOARD_PAD);
  for (const a of [-BOARD.pad, BOARD.pad])
    k.box(
      a - BOARD.padA,
      a + BOARD.padA,
      -BOARD.padD,
      BOARD.padD,
      lift + t,
      lift + t + BOARD.padH,
      pad,
    );
  const lime = s.tinted(BOARD_LIME);
  const yellow = s.tinted(BOARD_YELLOW);
  const h0 = lift + BOARD.padH;
  const h1 = lift + t - BOARD.padH;
  for (const [e0, e1] of [
    [w - BOARD.inset, d1],
    [-d1, -w + BOARD.inset],
  ] as const) {
    k.box(0, BOARD.flat, e0, e1, h0, h1, lime);
    k.box(-BOARD.flat, 0, e0, e1, h0, h1, yellow);
  }
};

/** The cloud's body: a bright golden yellow. */
const CLOUD_GOLD: Rgb = [0.95, 0.8, 0.2];

/** The cloud's pale highlights on top. */
const CLOUD_TOP: Rgb = [0.98, 0.92, 0.55];

/** The cloud's warmer orange-yellow underside. */
const CLOUD_UNDER: Rgb = [0.9, 0.6, 0.15];

/** How much flatter than round a lump is: its half height over its radius. */
const SQUASH = 0.85;

/**
 * The cloud's lumps: `[a, d, r, rise]`, the lump's centre over the lift by
 * `SQUASH * r + rise`, so every lump's underside is at or over the lift and
 * the main one's is exactly on it. The body, about 1.3 m long and 0.9 m
 * wide, runs from the front (`+d`) back to about `d` -0.35, with two
 * smaller puffs on its top; the last nine lumps are the tail, about 0.6 m
 * long, shrinking, rising and swinging to `+a` towards the back (`-d`).
 */
const LUMPS: readonly (readonly [number, number, number, number])[] = [
  [0, 0.3, 0.3, 0],
  [0.18, 0.6, 0.24, 0.02],
  [-0.18, 0.58, 0.23, 0.02],
  [0, 0.74, 0.2, 0.03],
  [0.2, 0.08, 0.24, 0.02],
  [-0.2, 0.1, 0.24, 0.02],
  [0, -0.12, 0.24, 0.03],
  [0.08, 0.45, 0.18, 0.2],
  [-0.1, 0.12, 0.18, 0.17],
  [0.04, -0.36, 0.16, 0.06],
  [0.05, -0.43, 0.15, 0.07],
  [0.08, -0.49, 0.13, 0.1],
  [0.11, -0.56, 0.12, 0.14],
  [0.15, -0.62, 0.105, 0.19],
  [0.19, -0.69, 0.09, 0.25],
  [0.23, -0.76, 0.08, 0.31],
  [0.29, -0.82, 0.065, 0.38],
  [0.34, -0.89, 0.05, 0.46],
];

/** A lump from this radius up carries a pale crown; the tail's do not. */
const CAPPED = 0.17;

/**
 * The ring angles of a lump, in degrees from its equator, bottom to top:
 * the underside's warm band ends at `WARM_TO`, the pale crown of a big
 * lump starts at `PALE_FROM`.
 */
const RINGS = [-90, -55, -20, 15, 45, 70, 90] as const;

/** The ring where the warm underside gives way to gold. */
const WARM_TO = -20;

/** The ring where a big lump's gold gives way to its pale crown. */
const PALE_FROM = 45;

/** The facets around a big lump, and around a small one (under `CAPPED`). */
const LUMP_SIDES = { big: 10, small: 6 } as const;

/**
 * One lump: a squashed ball of rings at `RINGS`, its underside warm, its
 * middle gold and, on a big lump, its crown pale. The bands share their
 * rings, so they meet without a seam.
 */
function lump(
  k: Kit,
  s: Surfaces,
  a: number,
  d: number,
  h: number,
  r: number,
): void {
  const v = SQUASH * r;
  const ring = (deg: number): [number, number] => {
    const t = (deg * Math.PI) / 180;
    return [Math.abs(deg) === 90 ? 0 : r * Math.cos(t), h + v * Math.sin(t)];
  };
  const big = r >= CAPPED;
  const sides = big ? LUMP_SIDES.big : LUMP_SIDES.small;
  const band = (lo: number, hi: number): [number, number][] =>
    RINGS.filter((g) => g >= lo && g <= hi).map(ring);
  k.lathe(a, d, band(-90, WARM_TO), sides, s.tinted(CLOUD_UNDER));
  const gold = s.tinted(CLOUD_GOLD);
  if (big) {
    k.lathe(a, d, band(WARM_TO, PALE_FROM), sides, gold);
    k.lathe(a, d, band(PALE_FROM, 90), sides, s.tinted(CLOUD_TOP));
  } else {
    k.lathe(a, d, band(WARM_TO, 90), sides, gold);
  }
}

/** The flying cloud: `LUMPS` over its lift, the tail at the back. */
const flyingCloud: HeroRecipe = ({ k, s, variant, kind }) => {
  const { lift } = heroHalf(kind, variant);
  for (const [a, d, r, rise] of LUMPS)
    lump(k, s, a, d, lift + SQUASH * r + rise, r);
};

/** The floating kinds' recipes. */
export const FLOATER_RECIPES = {
  "question-block": questionBlock,
  hoverboard,
  "flying-cloud": flyingCloud,
} satisfies Record<
  Extract<HeroKind, "question-block" | "hoverboard" | "flying-cloud">,
  HeroRecipe
>;
