/**
 * The archetypes' free-standing furniture, each recipe centred on its
 * point with its front towards `+d`, inside its `FOOTPRINTS` size.
 *
 * A bridge has its curved command console and the captain's chair, a
 * council chamber its round table and chairs, an engineering bay its
 * generator and the pipe runs along its ceiling, an archive its shelf
 * rows of files, and a lab its island with a sink and fume hood and its
 * specimen tanks.
 *
 * Every kind comes in two or three variants (2.7 C2, `RECIPES`), picked by
 * `decor.variant` (absent reads as 0), and every piece of one kind in a
 * room shares its variant (2.7 C4). Variant 0 is the kind's first model,
 * part for part (C1):
 *
 * - command console: the curved arc (0), a straight desk with two
 *   stations and a raised centre screen (1), a horseshoe of five
 *   segments (2);
 * - captain's chair: the pedestal chair (0), a command chair with a side
 *   console on each arm (1);
 * - round table: the lathe-turned table (0), a six-sided top on four
 *   legs (1);
 * - council chair: the plain chair (0), a bench-backed chair (1), a pod
 *   chair on a swivel foot (2);
 * - generator: the finned block (0), a lying turbine (1), twin stacks (2);
 * - pipe run: three pipes side by side (0), a bundle with valve wheels (1);
 * - shelf row: open shelves of files (0), card-drawer cabinets (1), open
 *   racks of boxes and tube rolls (2);
 * - lab island: the sink and fume hood (0), a microscope and centrifuge at
 *   the far end (1);
 * - specimen tank: the round tank (0), a square tank (1), a cluster of
 *   three thin tubes (2).
 *
 * Every variant keeps its kind's footprint, its host surfaces
 * (`DECOR_SURFACES`: the round table's places and under spots, the lab
 * island's bench) and the pipe run's box (C3). Each carries the room's
 * accent (C9) on one trim its recipe's doc names, drawn through `trim`
 * or `s.accent()`, which keep the part's own layer and flag, so on
 * variant 0 the accent re-tints a part it always had and adds none.
 */

import { createRng } from "../../core/seed";
import { FOOTPRINTS, PIPE_HALF, pipeLength } from "../../world/footprints";
import type { Decor, DecorKind } from "../../world/types";
import { DECAL_LIFT, frameForDecor, type Frame, type Kit } from "../kit";
import { accentTint, type Surface } from "../geometry";
import { hueToRgb } from "../looks";
import {
  HEADROOM,
  offset,
  profileAlong,
  shade,
  surfaces,
  yawed,
  type KitAt,
  type ModelContext,
  type Surfaces,
} from "./common";

/**
 * What a decor recipe gets, including which variant to draw: `RECIPES`
 * holds each kind's recipes in variant order, and `buildDecor` picks the
 * one `variant` names (2.7 C1, C2).
 */
interface Recipe {
  k: Kit;
  kitAt: KitAt;
  f: Frame;
  s: Surfaces;
  ctx: ModelContext;
  decor: Decor;
  variant: number;
}

/** Builds a piece of furniture at its point, turned with it. */
export function buildDecor(
  kitAt: KitAt,
  decor: Decor,
  ctx: ModelContext,
): void {
  const f = frameForDecor(decor);
  const variant = decor.variant ?? 0;
  const recipes = RECIPES[decor.kind];
  const recipe = recipes[variant] ?? recipes[0];
  recipe?.({
    k: kitAt(f),
    kitAt,
    f,
    s: surfaces(ctx.look),
    ctx,
    decor,
    variant,
  });
}

/** How far below the ceiling a pipe run's pipes hang (to their axis). */
export const PIPE_DROP = 0.35;

/**
 * How long a pipe run is (`world/footprints.ts`, E3), re-exported so the
 * models' index and their tests keep one import path.
 */
export { pipeLength };

/** Half a decor piece's footprint, width along and depth. */
function halves(kind: DecorKind): [number, number] {
  const size = FOOTPRINTS.decor[kind];
  return size ? [size.width / 2, size.depth / 2] : [0, 0];
}

/**
 * A part in the room's accent (2.7 C9) that keeps `surface`'s own layer
 * and flag: the accent mark in place of its tint, so a metal trim stays
 * metal and a variant 0 part that takes the accent is a pure re-tint.
 */
function trim(surface: Surface): Surface {
  return { ...surface, tint: accentTint(1) };
}

/** The command console: one segment's width and the wings' angle. */
const CONSOLE_CENTRE = 0.55;
const CONSOLE_WING = 0.9;
const CONSOLE_YAW = (20 * Math.PI) / 180;
/** A segment's depth, how far its back sits behind the piece's centre. */
const CONSOLE_DEPTH = 0.5;
const CONSOLE_SETBACK = 0.3;
/** The deck: its front edge height, its back edge depth and height. */
const DECK_FRONT = 0.7;
const DECK_BACK_D = 0.2;
const DECK_BACK = 0.92;
/** The screen housing at the back: its front depth and top; the screen. */
const HOUSING_D = 0.12;
const HOUSING_TOP = 1.25;
const SCREEN = [0.97, 1.2] as const;
/** The toggles: rows up the deck and their pitch along it. */
const TOGGLE_ROWS = 2;
const TOGGLE_PITCH = 0.1;

/**
 * One console segment in frame `sf`, from `a0` to `a1` along it, its back
 * on the frame's origin line and its front `CONSOLE_DEPTH` out: the body
 * (a sloped deck under a screen housing), the kick strip along its foot,
 * the glowing screen on the housing and two rows of toggles up the deck.
 * The accent (2.7 C9) goes on the kick strip when `underScreen` is false
 * (variant 0, which has no trim of its own), and on a trim under the
 * screen when it is true (every other variant), the strip then dark.
 */
function consoleSegment(
  kitAt: KitAt,
  sf: Frame,
  a0: number,
  a1: number,
  s: Surfaces,
  screen: Surface,
  underScreen: boolean,
) {
  const k = kitAt(sf);
  profileAlong(
    kitAt,
    sf,
    [
      [0, 0],
      [CONSOLE_DEPTH, 0],
      [CONSOLE_DEPTH, DECK_FRONT],
      [DECK_BACK_D, DECK_BACK],
      [HOUSING_D, DECK_BACK],
      [HOUSING_D, HOUSING_TOP],
      [0, HOUSING_TOP],
    ],
    a0,
    a1,
    s.body,
  );
  k.box(
    a0 + 0.02,
    a1 - 0.02,
    CONSOLE_DEPTH - 0.02,
    CONSOLE_DEPTH + 0.001,
    0.02,
    0.1,
    underScreen ? s.dark : trim(s.dark),
  );
  k.panel(
    a0 + 0.08,
    a1 - 0.08,
    HOUSING_D + DECAL_LIFT,
    SCREEN[0],
    SCREEN[1],
    screen,
  );
  if (underScreen) screenTrim(k, a0 + 0.08, a1 - 0.08, HOUSING_D, SCREEN[0], s);
  // Rows of toggles up the deck.
  for (let row = 0; row < TOGGLE_ROWS; row++) {
    const d = CONSOLE_DEPTH - 0.08 - row * 0.12;
    const h =
      DECK_FRONT +
      ((CONSOLE_DEPTH - d) / (CONSOLE_DEPTH - DECK_BACK_D)) *
        (DECK_BACK - DECK_FRONT);
    for (let a = a0 + 0.1; a + 0.05 < a1 - 0.05; a += TOGGLE_PITCH) {
      k.box(
        a,
        a + 0.04,
        d - 0.02,
        d + 0.02,
        h - 0.02,
        h + 0.025,
        row === 0 ? s.metal : s.dark,
      );
    }
  }
}

/**
 * The accent trim under a screen (2.7 C9): a thin strip `a0..a1` on the
 * face at depth `face`, just under the screen's lower edge `h0`.
 */
function screenTrim(
  k: Kit,
  a0: number,
  a1: number,
  face: number,
  h0: number,
  s: Surfaces,
) {
  k.box(a0, a1, face, face + 0.012, h0 - 0.035, h0 - 0.01, trim(s.metal));
}

/**
 * Command console (variant 0): an arc of three console segments, the
 * middle one square to the front and the two wings swung forward, each
 * with a sloped deck of toggles and a screen housing with its glowing
 * screen. Its accent is the three kick strips.
 */
function commandConsole({ kitAt, f, s, ctx }: Recipe) {
  const back = offset(f, 0, -CONSOLE_SETBACK);
  const screen = s.glow(shade(ctx.look.palette.screenText, 0.7));
  const segment = (sf: Frame, a0: number, a1: number) =>
    consoleSegment(kitAt, sf, a0, a1, s, screen, false);
  segment(back, -CONSOLE_CENTRE, CONSOLE_CENTRE);
  segment(yawed(back, CONSOLE_CENTRE, 0, CONSOLE_YAW), 0, CONSOLE_WING);
  segment(yawed(back, -CONSOLE_CENTRE, 0, -CONSOLE_YAW), -CONSOLE_WING, 0);
}

/**
 * The straight console: the desk's half length and depth, its back line,
 * the deck and housing heights, the two stations' screens and the raised
 * centre screen on its stand.
 */
const STRAIGHT = {
  half: 1.45,
  back: -0.45,
  depth: 0.75,
  deckFront: 0.72,
  deckBackD: 0.32,
  deck: 0.9,
  housingD: 0.12,
  housingTop: 1.15,
  /** A station screen runs from `inner` to `outer` either side of the middle. */
  inner: 0.42,
  outer: 1.3,
  screen: [0.96, 1.1],
  /** The raised screen: its half width, its housing's height and depth. */
  raisedHalf: 0.45,
  raised: [1.28, 1.85],
  raisedD: [0.02, 0.12],
  stand: 0.06,
} as const;

/**
 * Straight console (variant 1): one long straight desk, square to the
 * piece, with two stations (a screen and rows of toggles each) either side
 * of a raised centre screen on a stand over the middle. Nothing is yawed.
 * Its accent is a trim under each of its three screens.
 */
function straightConsole({ kitAt, f, s, ctx }: Recipe) {
  const C = STRAIGHT;
  const bf = offset(f, 0, C.back);
  const k = kitAt(bf);
  const screen = s.glow(shade(ctx.look.palette.screenText, 0.7));
  profileAlong(
    kitAt,
    bf,
    [
      [0, 0],
      [C.depth, 0],
      [C.depth, C.deckFront],
      [C.deckBackD, C.deck],
      [C.housingD, C.deck],
      [C.housingD, C.housingTop],
      [0, C.housingTop],
    ],
    -C.half,
    C.half,
    s.body,
  );
  k.box(
    -C.half + 0.02,
    C.half - 0.02,
    C.depth - 0.02,
    C.depth + 0.001,
    0.02,
    0.1,
    s.dark,
  );
  for (const dir of [-1, 1]) {
    const [a0, a1] = dir < 0 ? [-C.outer, -C.inner] : [C.inner, C.outer];
    k.panel(a0, a1, C.housingD + DECAL_LIFT, C.screen[0], C.screen[1], screen);
    screenTrim(k, a0, a1, C.housingD, C.screen[0], s);
    for (let row = 0; row < 3; row++) {
      const d = C.depth - 0.08 - row * 0.12;
      const h =
        C.deckFront +
        ((C.depth - d) / (C.depth - C.deckBackD)) * (C.deck - C.deckFront);
      for (let a = a0 + 0.04; a + 0.04 < a1; a += TOGGLE_PITCH) {
        k.box(
          a,
          a + 0.04,
          d - 0.02,
          d + 0.02,
          h - 0.02,
          h + 0.025,
          row === 0 ? s.metal : s.dark,
        );
      }
    }
  }
  // The stand and the raised centre screen's housing over the middle.
  k.box(
    -C.stand,
    C.stand,
    0.02,
    C.housingD - 0.02,
    C.housingTop,
    C.raised[0],
    s.metal,
  );
  k.box(
    -C.raisedHalf,
    C.raisedHalf,
    C.raisedD[0],
    C.raisedD[1],
    C.raised[0],
    C.raised[1],
    s.body,
  );
  const [r0, r1] = [-C.raisedHalf + 0.05, C.raisedHalf - 0.05];
  k.panel(
    r0,
    r1,
    C.raisedD[1] + DECAL_LIFT,
    C.raised[0] + 0.07,
    C.raised[1] - 0.05,
    screen,
  );
  screenTrim(k, r0, r1, C.raisedD[1], C.raised[0] + 0.07, s);
}

/**
 * The horseshoe console: its back line, the middle segment's half width,
 * and each side's two wings, their length and yaw, the second swung
 * further round than the first.
 */
const HORSESHOE = {
  back: -0.5,
  centre: 0.4,
  wings: [
    [0.55, (35 * Math.PI) / 180],
    [0.5, (70 * Math.PI) / 180],
  ],
} as const;

/**
 * Horseshoe console (variant 2): five segments on a deeper curve, the
 * middle one square to the front and two wings a side, each swung further
 * forward than the last, so the console wraps round the sides of the
 * chair in front of it. Every segment has its own screen, and its accent
 * is a trim under each of the five.
 */
function horseshoeConsole({ kitAt, f, s, ctx }: Recipe) {
  const H = HORSESHOE;
  const back = offset(f, 0, H.back);
  const screen = s.glow(shade(ctx.look.palette.screenText, 0.7));
  consoleSegment(kitAt, back, -H.centre, H.centre, s, screen, true);
  for (const dir of [-1, 1]) {
    let [a, d] = [dir * H.centre, 0];
    for (const [length, yaw] of H.wings) {
      const sf = yawed(back, a, d, dir * yaw);
      const [w0, w1] = dir < 0 ? [-length, 0] : [0, length];
      consoleSegment(kitAt, sf, w0, w1, s, screen, true);
      a += dir * length * Math.cos(yaw);
      d += length * Math.sin(yaw);
    }
  }
}

/**
 * A chair of seat half width `w` facing `+d`, its seat top at `seat`: the
 * seat in `cushion` (the room's accent, 2.7 C9), then the back in
 * `upholstery`.
 */
function chair(
  k: Kit,
  w: number,
  seat: number,
  back: number,
  upholstery: Surface,
  cushion: Surface,
) {
  k.bevelBox(-w, w, -w, w, seat - 0.08, seat, 0.03, cushion);
  k.bevelBox(-w, w, -w, -w + 0.08, seat, back, 0.03, upholstery);
}

/** The captain's chair: pedestal, seat, back and armrests. */
const CAPTAIN = {
  foot: 0.3,
  footHeight: 0.08,
  column: 0.07,
  seatHalf: 0.3,
  seat: 0.52,
  back: 1.3,
  arm: [0.64, 0.7],
  armWidth: 0.08,
} as const;

/**
 * Captain's chair (variant 0): a high-backed chair on a pedestal, with
 * armrests and a small glowing control pad on the right arm. Its accent
 * is the seat.
 */
function captainChair({ k, s, ctx }: Recipe) {
  const C = CAPTAIN;
  const leather = s.tinted(shade(ctx.look.palette.metal, 0.5));
  const w = C.seatHalf;
  const [r0, r1] = C.arm;
  k.cylinder(0, 0, 0, C.footHeight, C.foot, 12, s.dark);
  k.cylinder(0, 0, C.footHeight, C.seat - 0.12, C.column, 10, s.metal);
  chair(k, w, C.seat, C.back, leather, s.accent());
  for (const dir of [-1, 1]) {
    const [a0, a1] = dir < 0 ? [-w - C.armWidth, -w] : [w, w + C.armWidth];
    k.box(a0 + 0.02, a1 - 0.02, 0.05, 0.1, C.seat, r0, s.metal);
    k.bevelBox(a0, a1, -0.25, 0.25, r0, r1, 0.02, leather);
  }
  k.box(
    w + 0.01,
    w + C.armWidth - 0.01,
    0.1,
    0.2,
    r1,
    r1 + 0.01,
    s.glow(ctx.look.palette.door),
  );
}

/**
 * The command chair: its square plinth and post, a narrower seat, the
 * taller back and headrest, and the side consoles on the arm rests (their
 * width outside the seat, depth span and top).
 */
const COMMAND_CHAIR = {
  plinth: 0.26,
  plinthTop: 0.1,
  post: 0.08,
  seatHalf: 0.24,
  seat: 0.5,
  back: 1.55,
  headrest: [1.25, 1.48],
  console: 0.14,
  consoleD: [-0.28, 0.36],
  consoleTop: 0.7,
  support: 0.06,
} as const;

/**
 * Command chair (variant 1): a taller back with a padded headrest on a
 * square plinth and post, and a side console on each arm rest, each with
 * a small glowing panel and a row of keys. Its accent is the seat.
 */
function commandChair({ k, s, ctx }: Recipe) {
  const C = COMMAND_CHAIR;
  const leather = s.tinted(shade(ctx.look.palette.metal, 0.5));
  const w = C.seatHalf;
  const p = C.plinth;
  k.bevelBox(-p, p, -p, p, 0, C.plinthTop, 0.02, s.dark);
  k.box(-C.post, C.post, -C.post, C.post, C.plinthTop, C.seat - 0.08, s.metal);
  chair(k, w, C.seat, C.back, leather, s.accent());
  k.bevelBox(
    -0.15,
    0.15,
    -w + 0.08,
    -w + 0.14,
    C.headrest[0],
    C.headrest[1],
    0.02,
    leather,
  );
  const lights = [ctx.look.palette.door, ctx.look.palette.screenText];
  for (const dir of [-1, 1]) {
    const [a0, a1] = dir < 0 ? [-w - C.console, -w] : [w, w + C.console];
    const [d0, d1] = C.consoleD;
    const top = C.consoleTop;
    k.box(
      a0 + 0.04,
      a1 - 0.04,
      -C.support,
      C.support,
      C.seat - 0.08,
      top - 0.1,
      s.metal,
    );
    k.bevelBox(a0, a1, d0, d1, top - 0.1, top, 0.015, s.body);
    k.box(
      a0 + 0.02,
      a1 - 0.02,
      0.05,
      0.25,
      top,
      top + 0.01,
      s.glow(lights[dir < 0 ? 0 : 1] ?? ctx.look.palette.door),
    );
    for (let d = d0 + 0.05; d + 0.04 < 0.02; d += 0.07) {
      k.box(a0 + 0.03, a1 - 0.03, d, d + 0.04, top, top + 0.015, s.dark);
    }
  }
}

/** The round table: top height, foot, column, the glowing disc. */
const ROUND_TABLE = {
  top: 0.78,
  foot: 0.5,
  column: 0.18,
  flare: 0.64,
  disc: 0.4,
  sides: 24,
} as const;

/**
 * Round table (variant 0): a lathe-turned table on a flared foot, a rim
 * ring round its edge and a glowing disc set into the middle of the top.
 * Its accent is the rim ring.
 */
function roundTable({ k, s, ctx }: Recipe) {
  const T = ROUND_TABLE;
  const [r] = halves("round-table");
  const top = T.top;
  k.lathe(
    0,
    0,
    [
      [0, 0],
      [T.foot, 0],
      [T.foot, 0.05],
      [T.column, 0.12],
      [T.column, T.flare],
      [r - 0.1, top - 0.06],
      [r - 0.06, top],
      [T.disc + 0.02, top],
      [0, top],
    ],
    T.sides,
    s.body,
  );
  k.ring(0, 0, top - 0.03, r - 0.06, 0.03, 6, T.sides, trim(s.metal), "up");
  k.cylinder(
    0,
    0,
    top,
    top + 0.01,
    T.disc,
    T.sides,
    s.glow(ctx.look.palette.door),
  );
}

/**
 * The six-sided table: the top's corner radius (a flat side faces each
 * way the frame is square to, so the four places at `DECOR_SURFACES`
 * lie on the top), the top slab, the rim band under its edge, the legs on
 * the diagonals clear of the under spots, and the glowing centre ring.
 */
const HEX_TABLE = {
  corner: 1.18,
  slab: 0.04,
  band: [0.68, 0.74],
  bandDepth: 0.04,
  leg: 0.62,
  legHalf: 0.04,
  ring: 0.36,
  ringTube: 0.025,
  plate: 0.3,
} as const;

/**
 * Six-sided table (variant 1): a six-sided top on four square legs on the
 * diagonals, a rim band round the top's edge (its accent) and a glowing
 * ring set round a dark centre plate. The legs stand on the diagonals and
 * the band stays above the under spots' free height, so the four places
 * and the floor under them stay clear.
 */
function hexTable({ kitAt, f, k, s, ctx }: Recipe) {
  const T = HEX_TABLE;
  const top = ROUND_TABLE.top;
  const R = T.corner;
  // Yawed a twelfth of a turn, so a flat side (not a corner) faces +d.
  const hex = kitAt(yawed(f, 0, 0, Math.PI / 6));
  hex.lathe(
    0,
    0,
    [
      [0, top - T.slab],
      [R, top - T.slab],
      [R, top],
      [0, top],
    ],
    6,
    s.body,
  );
  const [b0, b1] = T.band;
  const inner = R - T.bandDepth;
  hex.lathe(
    0,
    0,
    [
      [inner, b0],
      [R, b0],
      [R, b1],
      [inner, b1],
      [inner, b0],
    ],
    6,
    trim(s.metal),
  );
  for (const a of [-T.leg, T.leg])
    for (const d of [-T.leg, T.leg])
      k.box(
        a - T.legHalf,
        a + T.legHalf,
        d - T.legHalf,
        d + T.legHalf,
        0,
        top - T.slab,
        s.metal,
      );
  k.cylinder(0, 0, top, top + 0.008, T.plate, 18, s.dark);
  k.ring(
    0,
    0,
    top + 0.01,
    T.ring,
    T.ringTube,
    6,
    24,
    s.glow(ctx.look.palette.door),
    "up",
  );
}

/** The council chair: seat half width and height, back, leg side. */
const COUNCIL_CHAIR = {
  seatHalf: 0.24,
  seat: 0.46,
  back: 0.95,
  leg: 0.04,
} as const;

/** Council chair (variant 0): a plain chair on four legs; its accent is the seat. */
function councilChair({ k, s, ctx }: Recipe) {
  const { seatHalf: w, seat, back, leg } = COUNCIL_CHAIR;
  for (const a of [-w, w - leg]) {
    for (const d of [-w, w - leg]) {
      k.box(a, a + leg, d, d + leg, 0, seat - 0.08, s.metal);
    }
  }
  chair(
    k,
    w,
    seat,
    back,
    s.tinted(shade(ctx.look.palette.machine, 0.85)),
    s.accent(),
  );
}

/**
 * The bench-backed chair: the seat's half width and height, the two
 * side frames it stands on, and the wide flat back behind it (half width,
 * depth span, bottom and top) on its two brackets.
 */
const BENCH_CHAIR = {
  seatHalf: 0.22,
  seat: 0.46,
  side: 0.035,
  backHalf: 0.34,
  backD: [-0.34, -0.29],
  back: [0.5, 1.05],
} as const;

/**
 * Bench-backed chair (variant 1): a seat on two solid side frames, and
 * behind it a wide flat back, wider than the seat, on two brackets, like
 * a pew's. Its accent is the seat.
 */
function benchChair({ k, s, ctx }: Recipe) {
  const C = BENCH_CHAIR;
  const w = C.seatHalf;
  const upholstery = s.tinted(shade(ctx.look.palette.machine, 0.85));
  for (const dir of [-1, 1]) {
    const [a0, a1] = dir < 0 ? [-w, -w + C.side] : [w - C.side, w];
    k.box(a0, a1, -w, w, 0, C.seat - 0.08, s.metal);
    k.box(a0, a1, C.backD[1], -w, 0.3, C.back[0] + 0.1, s.metal);
  }
  k.bevelBox(-w, w, -w, w, C.seat - 0.08, C.seat, 0.03, s.accent());
  const [d0, d1] = C.backD;
  k.bevelBox(
    -C.backHalf,
    C.backHalf,
    d0,
    d1,
    C.back[0],
    C.back[1],
    0.02,
    upholstery,
  );
  k.box(
    -C.backHalf + 0.03,
    C.backHalf - 0.03,
    d1,
    d1 + 0.01,
    C.back[1] - 0.07,
    C.back[1] - 0.03,
    s.dark,
  );
}

/**
 * The pod chair: the swivel foot and stem, the shell's profile (a bowl
 * revolved round the stem) and the curved back rising from its rear rim
 * (its radius, panel count, arc and top).
 */
const POD_CHAIR = {
  foot: 0.28,
  stem: 0.05,
  shell: [
    [0, 0.26],
    [0.18, 0.28],
    [0.29, 0.34],
    [0.32, 0.44],
    [0.32, 0.48],
    [0.28, 0.48],
    [0.25, 0.42],
    [0, 0.4],
  ],
  cushion: 0.24,
  backRadius: 0.3,
  backPanels: 5,
  backArc: [(125 * Math.PI) / 180, (235 * Math.PI) / 180],
  backTop: 1.0,
  backThick: 0.025,
} as const;

/**
 * Pod chair (variant 2): a rounded bowl shell (a lathe) on a swivel foot
 * and stem, a round cushion in the bowl (its accent), and a curved back of
 * narrow panels rising round the shell's rear half.
 */
function podChair({ kitAt, f, k, s, ctx }: Recipe) {
  const C = POD_CHAIR;
  const shell = s.tinted(shade(ctx.look.palette.machine, 0.85));
  k.cylinder(0, 0, 0, 0.04, C.foot, 16, s.dark);
  k.cylinder(0, 0, 0.04, 0.27, C.stem, 10, s.metal);
  k.lathe(0, 0, C.shell, 16, shell);
  k.cylinder(0, 0, 0.4, 0.45, C.cushion, 16, s.accent());
  const [t0, t1] = C.backArc;
  const step = (t1 - t0) / C.backPanels;
  const half = C.backRadius * Math.sin(step / 2) + 0.004;
  for (let i = 0; i < C.backPanels; i++) {
    const t = t0 + (i + 0.5) * step;
    const pf = yawed(
      f,
      C.backRadius * Math.sin(t),
      C.backRadius * Math.cos(t),
      -t,
    );
    kitAt(pf).box(
      -half,
      half,
      -C.backThick,
      C.backThick,
      0.44,
      C.backTop,
      shell,
    );
  }
}

/** The generator: block height, fins, the core on top. */
const GENERATOR = {
  top: 1.6,
  fin: 0.3,
  fins: 8,
  finSpan: [0.2, 1.4],
  core: 0.3,
  coreRadius: 0.25,
} as const;

/**
 * Generator (variant 0): a heavy block with cooling fins down both sides,
 * a caged glowing core on top and a gauge panel on the front. Its accent
 * is the band round the core.
 */
function generator({ k, s, decor }: Recipe) {
  const G = GENERATOR;
  const [hw, hd] = halves("generator");
  const body = hw - G.fin;
  k.bevelBox(-body, body, -hd + 0.1, hd - 0.1, 0, G.top, 0.05, s.body);
  k.bevelBox(
    -hw + 0.05,
    hw - 0.05,
    -hd + 0.05,
    hd - 0.05,
    0,
    0.12,
    0.03,
    s.dark,
  );
  for (const dir of [-1, 1]) {
    for (let i = 0; i < G.fins; i++) {
      const d = -hd + 0.3 + i * ((2 * hd - 0.6) / (G.fins - 1));
      const [a0, a1] = dir < 0 ? [-hw + 0.08, -body] : [body, hw - 0.08];
      k.box(a0, a1, d - 0.02, d + 0.02, G.finSpan[0], G.finSpan[1], s.metal);
    }
  }
  const core = hueToRgb(createRng(decor.seed).range(170, 210), 0.9, 0.55);
  const coreTop = G.top + G.core;
  k.cylinder(0, 0, G.top, coreTop, G.coreRadius, 12, s.glow(core));
  k.ring(
    0,
    0,
    (G.top + coreTop) / 2,
    G.coreRadius + 0.02,
    0.03,
    6,
    12,
    trim(s.dark),
    "up",
  );
  k.cylinder(0, 0, coreTop, coreTop + 0.08, G.coreRadius + 0.07, 12, s.metal);
  // The gauge panel on the front face.
  k.bevelBox(-0.4, 0.4, hd - 0.1, hd - 0.05, 0.8, 1.2, 0.01, s.dark);
  for (const a of [-0.25, 0, 0.25]) {
    k.box(a - 0.07, a + 0.07, hd - 0.05, hd - 0.045, 0.95, 1.08, s.glow(core));
  }
}

/**
 * The lying turbine: its skid, the two saddles (along, half width, top),
 * the drum (its axis height, radius and span along), the accent band on
 * it, the glowing end cap, the exhaust stack and the intake at the back.
 */
const TURBINE = {
  skid: 0.12,
  saddles: [-0.45, 0.35],
  saddleHalf: 0.1,
  axis: 0.8,
  radius: 0.55,
  drum: [-0.85, 0.6],
  bands: [-0.55, 0.25],
  accent: -0.15,
  capRadius: 0.45,
  cap: 0.08,
  stack: [-0.5, 1.95],
} as const;

/**
 * Lying turbine (variant 1): a horizontal drum on two saddles over a skid,
 * banded round (the middle band is its accent), a glowing end cap on the
 * drum's front end inside a dark flange, an exhaust stack on top and an
 * intake grille at its back end.
 */
function lyingTurbine({ k, s, decor }: Recipe) {
  const T = TURBINE;
  const [hw, hd] = halves("generator");
  const core = hueToRgb(createRng(decor.seed).range(170, 210), 0.9, 0.55);
  const [a0, a1] = T.drum;
  k.bevelBox(
    -hw + 0.05,
    hw - 0.05,
    -hd + 0.3,
    hd - 0.3,
    0,
    T.skid,
    0.03,
    s.dark,
  );
  for (const a of T.saddles)
    k.box(
      a - T.saddleHalf,
      a + T.saddleHalf,
      -T.radius + 0.1,
      T.radius - 0.1,
      T.skid,
      T.axis - T.radius + 0.1,
      s.metal,
    );
  k.cylinderAlong(a0, a1, 0, T.axis, T.radius, 16, s.body);
  for (const a of T.bands)
    k.cylinderAlong(
      a - 0.04,
      a + 0.04,
      0,
      T.axis,
      T.radius + 0.025,
      16,
      s.dark,
    );
  k.cylinderAlong(
    T.accent - 0.05,
    T.accent + 0.05,
    0,
    T.axis,
    T.radius + 0.03,
    16,
    trim(s.dark),
  );
  k.cylinderAlong(a1, a1 + 0.03, 0, T.axis, T.radius + 0.02, 16, s.dark);
  k.cylinderAlong(a1, a1 + T.cap, 0, T.axis, T.capRadius, 16, s.glow(core));
  k.cylinder(
    T.stack[0],
    0,
    T.axis + T.radius - 0.1,
    T.stack[1],
    0.12,
    12,
    s.metal,
  );
  k.cylinder(T.stack[0], 0, T.stack[1] - 0.06, T.stack[1], 0.15, 12, s.dark);
  k.box(-hw + 0.05, a0, -0.35, 0.35, T.axis - 0.35, T.axis + 0.35, s.dark);
  // A gauge box on the skid by the cap end, its dials lit.
  k.box(0.72, 0.92, 0.4, 0.62, T.skid, 0.9, s.body);
  for (const h of [0.55, 0.75])
    k.box(0.75, 0.89, 0.62, 0.625, h - 0.06, h + 0.06, s.glow(core));
}

/**
 * The twin stacks: the plinth, each stack's centre along, radius, bottom
 * and top, its accent band's height, the bridge pipe's height and radius,
 * and the sight glass on each stack's front.
 */
const TWIN_STACKS = {
  plinth: 0.15,
  stacks: [-0.5, 0.5],
  radius: 0.36,
  top: 1.75,
  band: 1.15,
  bridge: 1.5,
  bridgeRadius: 0.1,
  glass: [0.55, 1.0],
} as const;

/**
 * Twin stacks (variant 2): two tall cylinders on one plinth, each with a
 * domed cap, an accent band and a lit sight glass on its front, linked
 * high up by a bridge pipe with flanges and low down by a feed pipe.
 */
function twinStacks({ k, s, decor }: Recipe) {
  const T = TWIN_STACKS;
  const [hw] = halves("generator");
  const core = hueToRgb(createRng(decor.seed).range(170, 210), 0.9, 0.55);
  k.bevelBox(-hw + 0.05, hw - 0.05, -0.5, 0.5, 0, T.plinth, 0.03, s.dark);
  for (const a of T.stacks) {
    k.cylinder(a, 0, T.plinth, T.top, T.radius, 16, s.body);
    k.lathe(
      a,
      0,
      [
        [T.radius, T.top],
        [T.radius - 0.08, T.top + 0.1],
        [0.12, T.top + 0.14],
        [0, T.top + 0.14],
      ],
      16,
      s.metal,
    );
    k.ring(a, 0, T.band, T.radius, 0.03, 6, 16, trim(s.dark), "up");
    k.ring(a, 0, T.plinth + 0.05, T.radius, 0.03, 6, 16, s.dark, "up");
    k.box(
      a - 0.06,
      a + 0.06,
      T.radius - 0.06,
      T.radius + 0.01,
      T.glass[0],
      T.glass[1],
      s.glow(core),
    );
  }
  const [s0, s1] = T.stacks;
  k.cylinderAlong(s0, s1, 0, T.bridge, T.bridgeRadius, 12, s.metal);
  for (const a of [-0.2, 0.2])
    k.cylinderAlong(
      a - 0.03,
      a + 0.03,
      0,
      T.bridge,
      T.bridgeRadius + 0.03,
      12,
      s.dark,
    );
  k.cylinderAlong(s0, s1, -0.2, 0.4, 0.07, 10, s.metal);
}

/** The pipes of a run: depth of each axis and its radius. */
const PIPES: readonly (readonly [d: number, r: number])[] = [
  [-0.18, 0.07],
  [0, 0.05],
  [0.18, 0.07],
];
/**
 * The spacing of flanged joints and brackets, and how far in from each end
 * the brackets start. Their half-width across the run is `PIPE_HALF`
 * (`world/footprints.ts`), the box the span lines keep clear of.
 */
const PIPE_JOINT = 1.5;
const BRACKET_INSET = 0.3;

/**
 * Pipe run (variant 0): three pipes side by side along the ceiling with
 * flanges at every joint and brackets hanging them from the ceiling, as
 * long as the hall allows. Its accent is the brackets (each one's cross
 * bar and side posts; the hanger rod stays dark).
 */
function pipeRun({ k, s, ctx, decor }: Recipe) {
  const half = pipeLength(decor, ctx.hall) / 2;
  if (half <= 0) return;
  const h = ctx.ceiling - PIPE_DROP;
  for (const [d, r] of PIPES) {
    k.cylinderAlong(-half, half, d, h, r, 8, s.metal);
    for (let a = -half + 1; a < half - 0.5; a += PIPE_JOINT) {
      k.cylinderAlong(a - 0.04, a + 0.04, d, h, r + 0.02, 8, s.dark);
    }
  }
  const span = Math.max(0, 2 * half - 2 * BRACKET_INSET);
  const brackets = Math.max(1, Math.round(span / PIPE_JOINT));
  const w = PIPE_HALF;
  for (let i = 0; i <= brackets; i++) {
    const a = -half + BRACKET_INSET + (i * span) / brackets;
    k.box(a - 0.03, a + 0.03, -w, w, h - 0.1, h - 0.07, trim(s.dark));
    k.box(
      a - 0.02,
      a + 0.02,
      -0.02,
      0.02,
      h - 0.07,
      ctx.ceiling - HEADROOM,
      s.dark,
    );
    k.box(a - 0.03, a + 0.03, -w, -w + 0.03, h - 0.1, h + 0.1, trim(s.dark));
    k.box(a - 0.03, a + 0.03, w - 0.03, w, h - 0.1, h + 0.1, trim(s.dark));
  }
}

/**
 * The bundled pipes: each pipe's depth, its axis's height against the
 * run's hanging height and its radius, two below and one riding on them.
 */
const BUNDLE: readonly (readonly [d: number, dh: number, r: number])[] = [
  [-0.09, -0.05, 0.06],
  [0.09, -0.05, 0.06],
  [0, 0.07, 0.05],
];
/** A valve wheel: its radius, its tube's and how far out from the run's middle it stands. */
const VALVE = { radius: 0.07, tube: 0.012, face: 0.245 } as const;

/**
 * Pipe bundle (variant 1): three pipes bundled in a triangle, two below
 * and one riding on them, with flanges every `PIPE_JOINT`, a strap round
 * the bundle at each bracket on a hanger rod from the ceiling, and two
 * valves on the front pipe halfway out from the middle each way, their
 * hand wheels (the accent) facing the room.
 */
function pipeBundle({ k, s, ctx, decor }: Recipe) {
  const half = pipeLength(decor, ctx.hall) / 2;
  if (half <= 0) return;
  const h = ctx.ceiling - PIPE_DROP;
  for (const [d, dh, r] of BUNDLE) {
    k.cylinderAlong(-half, half, d, h + dh, r, 8, s.metal);
    for (let a = -half + 1; a < half - 0.5; a += PIPE_JOINT) {
      k.cylinderAlong(a - 0.04, a + 0.04, d, h + dh, r + 0.02, 8, s.dark);
    }
  }
  const span = Math.max(0, 2 * half - 2 * BRACKET_INSET);
  const brackets = Math.max(1, Math.round(span / PIPE_JOINT));
  for (let i = 0; i <= brackets; i++) {
    const a = -half + BRACKET_INSET + (i * span) / brackets;
    k.box(a - 0.03, a + 0.03, -0.18, 0.18, h - 0.14, h - 0.11, s.dark);
    k.box(a - 0.03, a + 0.03, -0.18, -0.15, h - 0.14, h + 0.06, s.dark);
    k.box(a - 0.03, a + 0.03, 0.15, 0.18, h - 0.14, h + 0.06, s.dark);
    k.box(a - 0.03, a + 0.03, -0.18, 0.18, h + 0.12, h + 0.15, s.dark);
    k.box(
      a - 0.02,
      a + 0.02,
      -0.02,
      0.02,
      h + 0.15,
      ctx.ceiling - HEADROOM,
      s.dark,
    );
  }
  const [, [pd, pdh] = [0.09, -0.05]] = BUNDLE;
  const ph = h + pdh;
  for (const a of [-half / 2, half / 2]) {
    k.box(
      a - 0.06,
      a + 0.06,
      pd - 0.02,
      pd + 0.08,
      ph - 0.07,
      ph + 0.07,
      s.metal,
    );
    k.box(
      a - 0.012,
      a + 0.012,
      pd + 0.08,
      VALVE.face,
      ph - 0.012,
      ph + 0.012,
      s.dark,
    );
    k.ring(
      a,
      VALVE.face,
      ph,
      VALVE.radius,
      VALVE.tube,
      6,
      12,
      trim(s.metal),
      "inward",
    );
    k.box(
      a - VALVE.radius,
      a + VALVE.radius,
      VALVE.face - 0.006,
      VALVE.face + 0.006,
      ph - 0.008,
      ph + 0.008,
      s.dark,
    );
    k.box(
      a - 0.008,
      a + 0.008,
      VALVE.face - 0.006,
      VALVE.face + 0.006,
      ph - VALVE.radius,
      ph + VALVE.radius,
      s.dark,
    );
  }
}

/** The shelf row: height, shelf levels, the file boxes' sizes and colours. */
const SHELF = {
  top: 2.2,
  levels: [0.05, 0.6, 1.15, 1.7],
  fileWidth: [0.14, 0.34],
  fileHeight: [0.22, 0.44],
  /** The share of file places left empty. */
  gaps: 0.12,
  hues: [0, 30, 55, 200, 220],
} as const;

/**
 * Shelf row (variant 0): a two-sided run of shelves, end panels and a
 * spine down the middle, with boxes of files on both faces of every shelf,
 * their widths and heights from the seed. Its accent is the two end
 * panels (the end caps).
 */
function shelfRow({ k, s, decor }: Recipe) {
  const [hw, hd] = halves("shelf-row");
  const rng = createRng(decor.seed);
  const top = SHELF.top;
  for (const a of [-hw, hw - 0.05])
    k.bevelBox(a, a + 0.05, -hd, hd, 0, top, 0.01, s.accent());
  k.box(-0.025, 0.025, -hd + 0.02, hd - 0.02, 0, top, s.body);
  k.box(-hw + 0.05, hw - 0.05, -0.01, 0.01, 0.05, top, s.dark);
  k.box(-hw, hw, -hd, hd, top, top + 0.04, s.metal);
  const colours = SHELF.hues.map((hue) => s.tinted(hueToRgb(hue, 0.25, 0.45)));
  for (const h of SHELF.levels) {
    k.box(-hw + 0.05, hw - 0.05, -hd + 0.02, hd - 0.02, h, h + 0.03, s.metal);
    for (const side of [-1, 1]) {
      for (const [a0, a1] of [
        [-hw + 0.08, -0.05],
        [0.05, hw - 0.08],
      ] as const) {
        let a = a0;
        while (a < a1 - 0.12) {
          const w = Math.min(rng.range(...SHELF.fileWidth), a1 - a);
          const height = rng.range(...SHELF.fileHeight);
          if (rng.next() > SHELF.gaps) {
            const [d0, d1] = side < 0 ? [-hd + 0.05, -0.02] : [0.02, hd - 0.05];
            k.box(
              a,
              a + w - 0.01,
              d0,
              d1,
              h + 0.03,
              h + 0.03 + height,
              rng.pick(colours),
            );
          }
          a += w;
        }
      }
    }
  }
}

/**
 * The card cabinets: their height, the drawer grid on each face (columns
 * across the run, rows up it, and the lowest row's bottom), the drawer
 * fronts' and pulls' depths off the carcass, and the end panels' width.
 */
const CARD_CABINET = {
  top: 1.45,
  columns: 10,
  rows: 5,
  bottom: 0.12,
  front: 0.02,
  pull: 0.02,
  end: 0.05,
} as const;

/**
 * Card-drawer cabinets (variant 1): a waist-high run of closed cabinets,
 * both faces covered in rows of small drawer fronts, each with a pull, a
 * metal top and the two end panels (the end caps, its accent).
 */
function cardCabinets({ k, s }: Recipe) {
  const C = CARD_CABINET;
  const [hw, hd] = halves("shelf-row");
  const face = hd - C.front - C.pull;
  for (const a of [-hw, hw - C.end])
    k.bevelBox(a, a + C.end, -hd, hd, 0, C.top + 0.05, 0.01, s.accent());
  k.box(-hw + C.end, hw - C.end, -face, face, 0, C.top, s.body);
  k.box(-hw + C.end, hw - C.end, -hd, hd, C.top, C.top + 0.04, s.metal);
  const a0 = -hw + C.end + 0.03;
  const pitchA = (2 * (hw - C.end - 0.03)) / C.columns;
  const pitchH = (C.top - 0.05 - C.bottom) / C.rows;
  for (const side of [-1, 1]) {
    const [f0, f1] =
      side < 0 ? [-face - C.front, -face] : [face, face + C.front];
    const [p0, p1] = side < 0 ? [-hd, -face - C.front] : [face + C.front, hd];
    for (let c = 0; c < C.columns; c++) {
      const a = a0 + c * pitchA;
      for (let r = 0; r < C.rows; r++) {
        const h = C.bottom + r * pitchH;
        k.box(
          a + 0.012,
          a + pitchA - 0.012,
          f0,
          f1,
          h + 0.012,
          h + pitchH - 0.012,
          s.panel,
        );
        const m = a + pitchA / 2;
        k.box(
          m - 0.04,
          m + 0.04,
          p0,
          p1,
          h + pitchH * 0.55,
          h + pitchH * 0.55 + 0.02,
          s.dark,
        );
      }
    }
  }
}

/**
 * The open racks: the posts' height and size, the deck levels, the boxes
 * on the lower decks (width and height ranges, from the seed) and the tube
 * rolls on the upper decks (their radius and lengths, from the seed).
 */
const OPEN_RACK = {
  top: 1.9,
  post: 0.04,
  levels: [0.1, 0.62, 1.14, 1.66],
  boxWidth: [0.3, 0.55],
  boxHeight: [0.2, 0.4],
  roll: 0.045,
  rollLength: [0.5, 0.85],
  hues: [30, 40, 200],
} as const;

/**
 * Open racks (variant 2): posts at the ends and the middle carrying four
 * open decks, cardboard boxes on the lower two and stacks of tube rolls
 * (drawings, rolled) lying along the upper two, a cap on each end frame's
 * top (the end caps, its accent).
 */
function openRacks({ k, s, decor }: Recipe) {
  const R = OPEN_RACK;
  const [hw, hd] = halves("shelf-row");
  const rng = createRng(decor.seed);
  const p = R.post;
  for (const a of [-hw, 0 - p / 2, hw - p])
    for (const d of [-hd, hd - p]) k.box(a, a + p, d, d + p, 0, R.top, s.metal);
  for (const a of [-hw, hw - 0.06])
    k.box(a, a + 0.06, -hd, hd, R.top, R.top + 0.04, trim(s.metal));
  for (const h of R.levels)
    k.box(-hw + p, hw - p, -hd + 0.01, hd - 0.01, h, h + 0.03, s.dark);
  const boxes = R.hues.map((hue) => s.tinted(hueToRgb(hue, 0.3, 0.5)));
  const rolls = [s.panel, s.tinted(hueToRgb(210, 0.2, 0.7))];
  R.levels.forEach((h, level) => {
    const base = h + 0.03;
    for (const [a0, a1] of [
      [-hw + p + 0.03, -p],
      [p, hw - p - 0.03],
    ] as const) {
      if (level < 2) {
        let a = a0;
        while (a < a1 - 0.2) {
          const w = Math.min(rng.range(...R.boxWidth), a1 - a);
          const height = rng.range(...R.boxHeight);
          k.box(
            a,
            a + w - 0.02,
            -hd + 0.05,
            hd - 0.05,
            base,
            base + height,
            rng.pick(boxes),
          );
          a += w;
        }
      } else {
        // A pyramid of rolls: three on the deck and two riding on them.
        const r = R.roll;
        const length = rng.range(...R.rollLength);
        const c = (a0 + a1) / 2;
        const at = [
          [-2 * r, base + r],
          [0, base + r],
          [2 * r, base + r],
          [-r, base + r * (1 + Math.sqrt(3))],
          [r, base + r * (1 + Math.sqrt(3))],
        ] as const;
        for (const [d, rh] of at)
          k.cylinderAlong(
            c - length / 2,
            c + length / 2,
            d,
            rh,
            r,
            8,
            rng.pick(rolls),
          );
      }
    }
  });
}

/** The lab island: top height, the sink's extent, tap, fume hood, duct. */
const LAB_ISLAND = {
  top: 0.96,
  sink: [0.55, 1.15],
  tap: 0.3,
  hoodEnd: -0.2,
  hoodTop: 1.9,
  duct: 2.4,
} as const;

/**
 * The island's body and its dark resin top, `hw` by `hd` about the
 * piece's centre, the top's surface at `top`: shared by every variant, so
 * the bench surface (`DECOR_SURFACES`) is the same on each.
 */
function islandBench(
  k: Kit,
  s: Surfaces,
  ctx: ModelContext,
  hw: number,
  hd: number,
  top: number,
) {
  k.bevelBox(
    -hw + 0.05,
    hw - 0.05,
    -hd + 0.05,
    hd - 0.05,
    0,
    top - 0.06,
    0.02,
    s.body,
  );
  k.bevelBox(
    -hw,
    hw,
    -hd,
    hd,
    top - 0.06,
    top,
    0.015,
    s.tinted(shade(ctx.look.palette.metal, 0.4)),
  );
}

/**
 * Lab island (variant 0): a bench island with a sink and its tap at one
 * end and a fume hood with a glowing work light and an exhaust duct at the
 * other. Its accent is the sink's raised rim (it has no edge band).
 */
function labIsland({ k, s, ctx }: Recipe) {
  const L = LAB_ISLAND;
  const [hw, hd] = halves("lab-island");
  const top = L.top;
  islandBench(k, s, ctx, hw, hd, top);
  // The sink: a raised steel rim, the dark basin and the tap.
  const [s0, s1] = L.sink;
  const tap = (s0 + s1) / 2;
  k.bevelBox(s0, s1, 0.05, 0.55, top, top + 0.03, 0.01, trim(s.metal));
  k.box(s0 + 0.05, s1 - 0.05, 0.1, 0.5, top + 0.03, top + 0.032, s.dark);
  k.cylinder(tap, 0.0, top, top + L.tap, 0.02, 8, s.metal);
  k.box(
    tap - 0.02,
    tap + 0.02,
    0.0,
    0.2,
    top + L.tap - 0.03,
    top + L.tap,
    s.metal,
  );
  // The fume hood: back, sides, top, the glowing work light and the duct.
  const [h0, h1] = [-hw + 0.1, L.hoodEnd];
  const hoodTop = L.hoodTop;
  k.bevelBox(h0, h1, -hd + 0.05, -hd + 0.15, top, hoodTop, 0.01, s.body);
  for (const a of [h0, h1 - 0.05])
    k.bevelBox(a, a + 0.05, -hd + 0.05, 0.35, top, hoodTop, 0.01, s.body);
  k.bevelBox(h0, h1, -hd + 0.05, 0.35, hoodTop - 0.15, hoodTop, 0.02, s.body);
  k.box(
    h0 + 0.1,
    h1 - 0.1,
    -0.3,
    0.2,
    hoodTop - 0.17,
    hoodTop - 0.15,
    s.glow(ctx.look.palette.lamp),
  );
  k.cylinder(
    (h0 + h1) / 2,
    -0.2,
    hoodTop,
    Math.min(L.duct, ctx.ceiling - HEADROOM),
    0.12,
    10,
    s.metal,
  );
}

/**
 * The instrument island: the edge band under the top on both long faces,
 * the microscope and the centrifuge on the far end (past the bench
 * surface, `a` over 0.5), and the reagent rack on the near end.
 */
const INSTRUMENT_ISLAND = {
  band: [0.12, 0.08],
  scope: 0.78,
  scopeTop: 0.47,
  centrifuge: 1.22,
  centrifugeRadius: 0.17,
  rack: [-1.4, -0.35],
  rackTop: 0.62,
} as const;

/**
 * Instrument island (variant 1): the same bench with no sink or hood; an
 * accent edge band runs under the top along both long faces, a microscope
 * (base, pillar, stage, head, objective and eyepieces) and a centrifuge
 * with a lit status lamp stand on the far end, and a two-shelf reagent
 * rack with bottles on the near end.
 */
function instrumentIsland({ k, s, ctx, decor }: Recipe) {
  const L = INSTRUMENT_ISLAND;
  const [hw, hd] = halves("lab-island");
  const top = LAB_ISLAND.top;
  islandBench(k, s, ctx, hw, hd, top);
  const face = hd - 0.05;
  for (const side of [-1, 1]) {
    const [d0, d1] = side < 0 ? [-face - 0.015, -face] : [face, face + 0.015];
    k.box(
      -hw + 0.08,
      hw - 0.08,
      d0,
      d1,
      top - L.band[0],
      top - L.band[1],
      trim(s.metal),
    );
  }
  // The microscope.
  const m = L.scope;
  const white = s.panel;
  k.bevelBox(m - 0.12, m + 0.12, -0.14, 0.12, top, top + 0.04, 0.01, white);
  k.box(m - 0.1, m - 0.04, -0.12, -0.04, top + 0.04, top + 0.36, white);
  k.box(m - 0.07, m + 0.08, -0.08, 0.08, top + 0.14, top + 0.16, s.dark);
  k.bevelBox(m - 0.1, m + 0.08, -0.12, 0.04, top + 0.3, top + 0.4, 0.01, white);
  k.cylinder(m, 0.0, top + 0.2, top + 0.3, 0.02, 8, s.metal);
  for (const d of [-0.1, -0.04])
    k.cylinder(m - 0.02, d, top + 0.4, top + L.scopeTop, 0.015, 8, s.dark);
  // The centrifuge, its lid domed, its status lamp lit.
  const c = L.centrifuge;
  const r = L.centrifugeRadius;
  k.cylinder(c, 0, top, top + 0.2, r, 16, white);
  k.lathe(
    c,
    0,
    [
      [r, top + 0.2],
      [r - 0.02, top + 0.24],
      [0.05, top + 0.26],
      [0, top + 0.26],
    ],
    16,
    s.metal,
  );
  k.box(
    c - 0.03,
    c + 0.03,
    r - 0.02,
    r + 0.01,
    top + 0.08,
    top + 0.12,
    s.glow(ctx.look.palette.lamp),
  );
  // The reagent rack on the near end, its bottles from the seed.
  const [r0, r1] = L.rack;
  const rng = createRng(decor.seed);
  for (const a of [r0, r1 - 0.03])
    k.box(a, a + 0.03, -0.12, 0.12, top, top + L.rackTop, s.metal);
  for (const h of [top + 0.02, top + 0.32, top + L.rackTop - 0.02])
    k.box(r0, r1, -0.12, 0.12, h - 0.02, h, s.metal);
  for (const h of [top + 0.02, top + 0.32])
    for (let a = r0 + 0.08; a < r1 - 0.06; a += 0.11) {
      const tint = hueToRgb(rng.range(0, 360), 0.5, 0.5);
      k.cylinder(a, 0, h, h + rng.range(0.12, 0.22), 0.035, 8, s.tinted(tint));
    }
}

/** The specimen tank: base, glass top, collar, cap and feed tubes. */
const SPECIMEN_TANK = {
  base: 0.25,
  glass: 1.6,
  collar: 0.95,
  cap: 1.8,
  tubes: 0.2,
  sides: 16,
} as const;

/**
 * Specimen tank (variant 0): a glass tank glowing with the liquid it
 * holds, on a base, with a cap, a collar ring and feed tubes on top. Its
 * accent is the collar ring (it has no ring on its cap).
 */
function specimenTank({ k, s, decor }: Recipe) {
  const T = SPECIMEN_TANK;
  const [hw] = halves("specimen-tank");
  const r = hw - 0.03;
  const g = r - 0.04;
  k.cylinder(0, 0, 0, T.base, r, T.sides, s.body);
  const liquid = hueToRgb(createRng(decor.seed).range(90, 180), 0.7, 0.5);
  k.lathe(
    0,
    0,
    [
      [0, T.base],
      [g - 0.04, T.base],
      [g, T.base + 0.25],
      [g, T.glass - 0.2],
      [g - 0.04, T.glass],
      [0, T.glass],
    ],
    T.sides,
    s.glow(liquid),
  );
  k.ring(0, 0, T.collar, g, 0.03, 6, T.sides, trim(s.metal), "up");
  k.cylinder(0, 0, T.glass, T.cap, r, T.sides, s.body);
  for (const a of [-0.15, 0.15]) {
    k.cylinder(a, 0, T.cap, T.cap + T.tubes, 0.025, 6, s.metal);
  }
}

/**
 * The square tank: the base's and the cap's half size and heights, the
 * liquid's half size, the corner posts, and the ring round the cap.
 */
const SQUARE_TANK = {
  half: 0.42,
  base: 0.22,
  liquid: 0.36,
  glass: 1.3,
  cap: 1.46,
  post: 0.05,
} as const;

/**
 * Square tank (variant 1): a square tank of glowing liquid on a square
 * base, framed by four corner posts and a frame at the foot, under a
 * square cap with a ring (its accent) round its lower edge and two feed
 * tubes on top.
 */
function squareTank({ k, s, decor }: Recipe) {
  const T = SQUARE_TANK;
  const w = T.half;
  const g = T.liquid;
  const liquid = hueToRgb(createRng(decor.seed).range(90, 180), 0.7, 0.5);
  k.bevelBox(-w, w, -w, w, 0, T.base, 0.02, s.body);
  k.box(-g, g, -g, g, T.base, T.glass, s.glow(liquid));
  for (const a of [-w, w - T.post])
    for (const d of [-w, w - T.post])
      k.box(a, a + T.post, d, d + T.post, T.base, T.glass, s.metal);
  k.box(-w, w, -w, w, T.base, T.base + 0.04, s.metal);
  k.bevelBox(-w, w, -w, w, T.glass, T.cap, 0.02, s.body);
  k.box(
    -w - 0.01,
    w + 0.01,
    -w - 0.01,
    w + 0.01,
    T.glass,
    T.glass + 0.04,
    trim(s.metal),
  );
  for (const a of [-0.15, 0.15])
    k.cylinder(a, 0, T.cap, T.cap + 0.2, 0.025, 6, s.metal);
}

/**
 * The tube cluster: the shared base, the three tubes' distance from the
 * middle and radius, their glass's top, and each tube's cap.
 */
const TUBE_CLUSTER = {
  base: 0.2,
  baseRadius: 0.42,
  spread: 0.2,
  tube: 0.11,
  glass: 1.5,
  cap: 1.6,
} as const;

/**
 * Tube cluster (variant 2): three thin glowing tubes, each its own core,
 * standing on one round base, each with a collar at its foot and a cap on
 * top ringed in the accent, and a manifold pipe over the middle joining
 * the three caps.
 */
function tubeCluster({ k, s, decor }: Recipe) {
  const T = TUBE_CLUSTER;
  const rng = createRng(decor.seed);
  k.cylinder(0, 0, 0, T.base, T.baseRadius, 18, s.body);
  for (let i = 0; i < 3; i++) {
    const t = (2 * Math.PI * i) / 3;
    const a = T.spread * Math.sin(t);
    const d = T.spread * Math.cos(t);
    const liquid = hueToRgb(rng.range(90, 180), 0.7, 0.5);
    k.cylinder(a, d, T.base, T.glass, T.tube, 12, s.glow(liquid));
    k.ring(a, d, T.base + 0.03, T.tube, 0.025, 6, 12, s.metal, "up");
    k.cylinder(a, d, T.glass, T.cap, T.tube + 0.02, 12, s.body);
    k.ring(
      a,
      d,
      T.glass + 0.02,
      T.tube + 0.02,
      0.02,
      6,
      12,
      trim(s.metal),
      "up",
    );
    k.box(
      a / 2 - 0.02,
      a / 2 + 0.02,
      d / 2 - 0.02,
      d / 2 + 0.02,
      T.cap - 0.06,
      T.cap - 0.02,
      s.metal,
    );
  }
  k.cylinder(0, 0, T.cap - 0.08, T.cap + 0.12, 0.05, 10, s.metal);
}

/**
 * Every decor kind's recipes in variant order (2.7 C2): entry `v` builds
 * variant `v`, entry 0 the kind's first model, part for part (2.7 C1).
 * Every variant keeps its kind's footprint, the host surfaces
 * (`DECOR_SURFACES`) and the pipe run's box (2.7 C3).
 */
const RECIPES: Record<DecorKind, readonly ((r: Recipe) => void)[]> = {
  "command-console": [commandConsole, straightConsole, horseshoeConsole],
  "captain-chair": [captainChair, commandChair],
  "round-table": [roundTable, hexTable],
  "council-chair": [councilChair, benchChair, podChair],
  generator: [generator, lyingTurbine, twinStacks],
  "pipe-run": [pipeRun, pipeBundle],
  "shelf-row": [shelfRow, cardCabinets, openRacks],
  "lab-island": [labIsland, instrumentIsland],
  "specimen-tank": [specimenTank, squareTank, tubeCluster],
};
