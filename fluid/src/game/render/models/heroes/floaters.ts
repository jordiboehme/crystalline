/**
 * The hovering heroes' recipes: the question block, the hoverboard and the
 * flying cloud. What they share is that they hover (C4): each is built at
 * its lift (`heroHalf`'s `lift`), so its lowest vertex is the height it
 * hovers at, while the instance itself stands at `y` 0. The block hangs
 * over the player's head at 2.3 m, the board level at 0.25 m and the
 * cloud's underside at 0.4 m. Only the block has a light: its four marks
 * breathe; the board and the cloud are unlit, as their originals are.
 *
 * Two marks are readable here. The block's (C12) is a "?" drawn as a pixel
 * picture, `QUESTION_MARK`, not a glyph of `PIXEL_FONT`, so the font still
 * refuses "?" and the block stays its only use. The board's is its
 * original's wordmark on its deck (`MARKS.boardLogo`, 2.6f C13), set in
 * the font as one two-line block on a yellow patch.
 *
 * The numbers each kind is built to are named in a table above its
 * recipe: `BLOCK` for the block, `BOARD` for the hoverboard, and `BODY`
 * and `TAIL` for the cloud, whose tail balls (`TAIL_BALLS`) are walked
 * out once from `TAIL`. Round parts use few facets, as in every batch.
 */

import type { HeroKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import { MARKS } from "../marks";
import { yawed, type Surfaces } from "../common";
import { heroHalf, type HeroRecipe } from "./common";
import { fit, pixelPanel, runsOf, textBlock } from "./pixels";

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

/** The mark's drop shadow: the rivets' own dark brown. */
const MARK_SHADOW: Rgb = BLOCK_RIVET;

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
 * faces:
 * - `face`: how far the edge-coloured core reaches;
 * - `plate`: how far the gold plate on each face stands out;
 * - `inset`: how far each plate stops short of the core's edges, so a band
 *   of the edge colour that wide frames every face;
 * - `rivet`: half the side of a dark corner rivet, and `rivetIn`: how far
 *   a rivet's centre sits in from its plate's corner;
 * - `px`: the side of one of the mark's square pixels.
 * The rivets and the marks stand `DECAL_LIFT` proud of the plates, out to
 * the 0.3 half width, the top and the lift exactly.
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

/** The board's back edge panels and the patch under its wordmark: yellow. */
const BOARD_YELLOW: Rgb = [0.98, 0.85, 0.1];

/**
 * The board's wordmark letters (`MARKS.boardLogo`): a magenta on the
 * yellow patch, as the original prints it (2.6f C13). No other part of
 * the board wears it, so a test can count the letters.
 */
export const DECK_LOGO_INK: Rgb = [0.85, 0.1, 0.55];

/**
 * The hoverboard's measures, in metres. The deck is `thick` thick and
 * `inset` narrower on each side than the footprint, whose last `inset`
 * the edge panels fill. Along its length it is flat out to `flat` from the
 * centre, then ramps up (the kick) to a level shelf, on which each round
 * end sits: a disc of the deck's half width and `sides` facets, centred
 * where the shelf ends. The disc is `seat` thinner than the shelf at top
 * and bottom, so the half of it over the shelf is hidden inside the deck
 * and no face of the two shares a plane. A foot pad `padH` thick lies on
 * each shelf, `padA` long and `padD` wide in half measures, and ends
 * exactly at the top. The edge panels follow the deck's profile, `edge`
 * inside its top and bottom. The wordmark's patch spans `logo` either way
 * of the middle along `a`, inside the flat.
 */
const BOARD = {
  thick: 0.035,
  inset: 0.015,
  flat: 0.12,
  sides: 16,
  seat: 0.001,
  padA: 0.05,
  padD: 0.07,
  padH: 0.004,
  edge: 0.004,
  logo: 0.11,
} as const;

/**
 * Lays the board's wordmark flat on its deck's top at height `h`: the
 * lines of `MARKS.boardLogo` as one block (`textBlock`) with one dark
 * pixel of border, fitted along `a` into `-half..half` and centred across
 * `d`, the first line at `-d` so the mark reads from the board's front
 * (`+d`). A frame cannot tilt, so no quad can face up; each run is a thin
 * `k.box` one `DECAL_LIFT` thick instead, the lit runs in `ink` and the
 * dark ones in `patch`, side by side, so the patch and the letters are one
 * flat layer with no face over another.
 */
function deckLogo(
  k: Kit,
  half: number,
  h: number,
  ink: Surface,
  patch: Surface,
): void {
  const block = textBlock(MARKS.boardLogo);
  const w = (block[0]?.length ?? 0) + 2;
  const rows = [".".repeat(w), ...block.map((r) => `.${r}.`), ".".repeat(w)];
  const { px, left } = fit(rows, -half, half, -half, half);
  const far = -(rows.length * px) / 2;
  for (const r of runsOf(rows))
    k.box(
      left + r.col * px,
      left + (r.col + r.len) * px,
      far + r.row * px,
      far + (r.row + 1) * px,
      h,
      h + DECAL_LIFT,
      r.ch === "#" ? ink : patch,
    );
}

/**
 * The hoverboard: a deck profile along `a`, flat in the middle and
 * kicking up to a level shelf at each end, extruded across `d`; a round
 * disc on each shelf for the rounded end; a black foot pad on each shelf;
 * and edge panels along both sides that follow the deck, lime towards
 * `+a` and yellow towards `-a`; on the flat middle between the pads, the
 * wordmark in magenta on a yellow patch (`deckLogo`). Level at its lift.
 */
const hoverboard: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw, d1, lift, top } = heroHalf(kind, variant);
  const t = BOARD.thick;
  const w = d1 - BOARD.inset;
  // Where each disc is centred, and where the shelf under its inner half
  // starts: the ramp ends there.
  const c = hw - w;
  const ramp = c - w;
  // The shelf's underside: the pads on it end at the top.
  const shelf = top - BOARD.padH - t;
  const pink = s.tinted(BOARD_PINK);
  const under = (x: number): number =>
    lift +
    (shelf - lift) *
      Math.min(1, Math.max(0, Math.abs(x) - BOARD.flat) / (ramp - BOARD.flat));
  // The deck's outline over `xs`, from `lo` over its underside to `hi`.
  const profile = (xs: readonly number[], lo: number, hi: number) => [
    ...xs.map((x): [number, number] => [x, under(x) + lo]),
    ...xs.map((x): [number, number] => [x, under(x) + hi]).reverse(),
  ];
  k.extrude(
    profile([-c, -ramp, -BOARD.flat, BOARD.flat, ramp, c], 0, t),
    -w,
    w,
    pink,
  );
  const pad = s.tinted(BOARD_PAD);
  const padAt = (c + ramp) / 2;
  for (const a of [-c, c])
    k.cylinder(
      a,
      0,
      shelf + BOARD.seat,
      shelf + t - BOARD.seat,
      w,
      BOARD.sides,
      pink,
    );
  for (const a of [-padAt, padAt])
    k.box(
      a - BOARD.padA,
      a + BOARD.padA,
      -BOARD.padD,
      BOARD.padD,
      shelf + t,
      top,
      pad,
    );
  const lime = s.tinted(BOARD_LIME);
  const yellow = s.tinted(BOARD_YELLOW);
  const lo = BOARD.edge;
  const hi = t - BOARD.edge;
  for (const [e0, e1] of [
    [w - BOARD.inset, d1],
    [-d1, -w + BOARD.inset],
  ] as const) {
    k.extrude(profile([0, BOARD.flat, ramp, c], lo, hi), e0, e1, lime);
    k.extrude(profile([-c, -ramp, -BOARD.flat, 0], lo, hi), e0, e1, yellow);
  }
  deckLogo(k, BOARD.logo, lift + t, s.tinted(DECK_LOGO_INK), yellow);
};

/** The cloud's body: a bright golden yellow. */
const CLOUD_GOLD: Rgb = [0.95, 0.8, 0.2];

/** The cloud's pale highlights on top. */
const CLOUD_TOP: Rgb = [0.98, 0.92, 0.55];

/** The cloud's warmer orange-yellow underside. */
const CLOUD_UNDER: Rgb = [0.9, 0.6, 0.15];

/**
 * How flat a body lump's underside is: the height of its lower half over
 * its radius. Every body lump's underside rests on the lift, so the body
 * has one flat bottom.
 */
const FLAT_UNDER = 0.35;

/**
 * The body's lumps: `[a, d, r, up]`, each a squashed ball of radius `r`
 * whose lower half is `FLAT_UNDER * r` high and whose rounded upper half
 * is `up` high. They overlap heavily, so the body reads as one broad, low
 * puff about 1.35 m long, 0.85 m wide and 0.45 m tall, highest in its
 * middle.
 */
const BODY: readonly (readonly [number, number, number, number])[] = [
  [0, 0.22, 0.3, 0.32],
  [0, 0.62, 0.28, 0.27],
  [0, -0.16, 0.27, 0.26],
  [0.22, 0.44, 0.22, 0.24],
  [-0.22, 0.4, 0.22, 0.25],
  [0.22, 0.02, 0.22, 0.23],
  [-0.22, 0.04, 0.22, 0.24],
];

/**
 * The tail: a chain of `count` small squashed balls overlapping so closely
 * (each `spacing` of its radius on from the last) that it reads as one
 * wisp, tapering from `r0` at the body's back (`start`, on the lift) to
 * `r1` at its tip over `length` metres. It heads to `-d`, turning by
 * `turn` radians towards `+a` and rising by `rise` as it goes, so it curls
 * up and to one side. A tail ball's halves are each `squash` of its radius
 * high.
 */
const TAIL = {
  start: [0.02, -0.28],
  length: 0.6,
  count: 32,
  r0: 0.2,
  r1: 0.03,
  turn: (100 * Math.PI) / 180,
  rise: 0.45,
  squash: 0.75,
} as const;

/**
 * The tail's balls, `[a, d, r, base]` (`base` over the lift), walked out
 * along the tail once: the radius shrinks geometrically, so a step of
 * the arc, proportional to the radius, keeps the overlap the same
 * everywhere.
 */
const TAIL_BALLS: readonly (readonly [number, number, number, number])[] =
  (() => {
    const q = TAIL.r1 / TAIL.r0;
    const out: [number, number, number, number][] = [];
    let [a, d] = TAIL.start;
    let prev = 0;
    for (let i = 0; i < TAIL.count; i++) {
      const f = i / (TAIL.count - 1);
      const arc = (TAIL.length * (1 - q ** f)) / (1 - q);
      // Walk from the last ball's arc to this one's in small steps.
      const steps = 8;
      for (let j = 1; j <= steps; j++) {
        const s0 = prev + ((arc - prev) * (j - 0.5)) / steps;
        const heading = TAIL.turn * (s0 / TAIL.length) ** 1.5;
        a += (Math.sin(heading) * (arc - prev)) / steps;
        d -= (Math.cos(heading) * (arc - prev)) / steps;
      }
      prev = arc;
      out.push([
        a,
        d,
        TAIL.r0 * q ** f,
        TAIL.rise * (arc / TAIL.length) ** 2.2,
      ]);
    }
    return out;
  })();

/**
 * How a ball of the cloud is faceted: `sides` facets around, and the ring
 * angles of its lower and upper halves in degrees, bottom to top. From
 * `pale` up, a ball wears the pale crown; a tail ball (`pale` null) has
 * none. A body ball is rounder, a tail ball plainer, so the tail's many
 * balls stay within the triangle aim.
 */
interface BallShape {
  sides: number;
  lower: readonly number[];
  upper: readonly number[];
  pale: number | null;
}

/** A body ball: 9 facets, three rings on top, a pale crown from 55 degrees. */
const BODY_BALL: BallShape = {
  sides: 9,
  lower: [-90, -45, 0],
  upper: [0, 30, 55, 90],
  pale: 55,
};

/** A tail ball: 5 facets, one ring in each half, all gold over its warm underside. */
const TAIL_BALL: BallShape = {
  sides: 5,
  lower: [-90, -40, 0],
  upper: [0, 50, 90],
  pale: null,
};

/**
 * One ball of the cloud: radius `r` around the vertical axis at `(a, d)`,
 * its underside at `base`, a lower half `low` high in the warm underside
 * colour and a rounded upper half `up` high in gold, with a pale crown if
 * its shape has one. The bands share their rings, so they meet without a
 * seam.
 */
function ball(
  k: Kit,
  s: Surfaces,
  [a, d, r]: readonly [number, number, number],
  base: number,
  low: number,
  up: number,
  shape: BallShape,
): void {
  const h = base + low;
  const ring = (deg: number): [number, number] => {
    const t = (deg * Math.PI) / 180;
    const x = Math.abs(deg) === 90 ? 0 : r * Math.cos(t);
    return [x, h + (deg < 0 ? low : up) * Math.sin(t)];
  };
  k.lathe(a, d, shape.lower.map(ring), shape.sides, s.tinted(CLOUD_UNDER));
  const upper = shape.upper.map(ring);
  const cut = shape.pale === null ? -1 : shape.upper.indexOf(shape.pale);
  if (cut < 0) {
    k.lathe(a, d, upper, shape.sides, s.tinted(CLOUD_GOLD));
    return;
  }
  k.lathe(a, d, upper.slice(0, cut + 1), shape.sides, s.tinted(CLOUD_GOLD));
  k.lathe(a, d, upper.slice(cut), shape.sides, s.tinted(CLOUD_TOP));
}

/** The flying cloud: the body's flat-bottomed puff on its lift, the tail at the back. */
const flyingCloud: HeroRecipe = ({ k, s, variant, kind }) => {
  const { lift } = heroHalf(kind, variant);
  for (const [a, d, r, up] of BODY)
    ball(k, s, [a, d, r], lift, FLAT_UNDER * r, up, BODY_BALL);
  for (const [a, d, r, rise] of TAIL_BALLS)
    ball(
      k,
      s,
      [a, d, r],
      lift + rise,
      TAIL.squash * r,
      TAIL.squash * r,
      TAIL_BALL,
    );
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
