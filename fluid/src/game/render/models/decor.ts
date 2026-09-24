/**
 * The archetypes' free-standing furniture, each recipe centred on its
 * point with its front towards `+d`, inside its `FOOTPRINTS` size.
 *
 * A bridge has its curved command console and the captain's chair, a
 * council chamber its round table and chairs, an engineering bay its
 * generator and the pipe runs along its ceiling, an archive its shelf
 * rows of files, and a lab its island with a sink and fume hood and its
 * specimen tanks.
 */

import { createRng } from "../../core/seed";
import { CELL } from "../../world/generate";
import { FOOTPRINTS } from "../../world/move";
import type { Decor, DecorKind, Rect } from "../../world/types";
import { frameForDecor, type Frame, type Kit } from "../kit";
import type { Surface } from "../geometry";
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

/** What a decor recipe gets. */
interface Recipe {
  k: Kit;
  kitAt: KitAt;
  f: Frame;
  s: Surfaces;
  ctx: ModelContext;
  decor: Decor;
}

/** Builds a piece of furniture at its point, turned with it. */
export function buildDecor(
  kitAt: KitAt,
  decor: Decor,
  ctx: ModelContext,
): void {
  const f = frameForDecor(decor);
  RECIPES[decor.kind]({
    k: kitAt(f),
    kitAt,
    f,
    s: surfaces(ctx.look),
    ctx,
    decor,
  });
}

/** How far below the ceiling a pipe run's pipes hang (to their axis). */
export const PIPE_DROP = 0.35;
/** The longest pipe run, and how much shorter than the hall it stays. */
const PIPE_MAX = 6;
const PIPE_CLEARANCE = 1;

/**
 * How long a pipe run is: at most 6 m, and 1 m shorter than the hall is
 * along the run (along x at turns 0 and 2, along the grid's y at 1 and 3),
 * so it never pokes through a wall.
 */
export function pipeLength(decor: Decor, hall: Rect): number {
  const cells = decor.turn % 2 === 0 ? hall.x1 - hall.x0 : hall.y1 - hall.y0;
  return Math.max(0, Math.min(PIPE_MAX, cells * CELL - PIPE_CLEARANCE));
}

/** Half a decor piece's footprint, width along and depth. */
function halves(kind: DecorKind): [number, number] {
  const size = FOOTPRINTS.decor[kind];
  return size ? [size.width / 2, size.depth / 2] : [0, 0];
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
 * Command console: an arc of three console segments, the middle one
 * square to the front and the two wings swung forward, each with a
 * sloped deck of toggles and a screen housing with its glowing screen.
 */
function commandConsole({ kitAt, f, s, ctx }: Recipe) {
  const back = offset(f, 0, -CONSOLE_SETBACK);
  const screen = s.glow(shade(ctx.look.palette.screenText, 0.7));
  const segment = (sf: Frame, a0: number, a1: number) => {
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
      s.dark,
    );
    k.panel(
      a0 + 0.08,
      a1 - 0.08,
      HOUSING_D + 0.001,
      SCREEN[0],
      SCREEN[1],
      screen,
    );
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
  };
  segment(back, -CONSOLE_CENTRE, CONSOLE_CENTRE);
  segment(yawed(back, CONSOLE_CENTRE, 0, CONSOLE_YAW), 0, CONSOLE_WING);
  segment(yawed(back, -CONSOLE_CENTRE, 0, -CONSOLE_YAW), -CONSOLE_WING, 0);
}

/** A chair of seat half width `w` facing `+d`, its seat top at `seat`. */
function chair(
  k: Kit,
  w: number,
  seat: number,
  back: number,
  upholstery: Surface,
) {
  k.bevelBox(-w, w, -w, w, seat - 0.08, seat, 0.03, upholstery);
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
 * Captain's chair: a high-backed chair on a pedestal, with armrests and a
 * small glowing control pad on the right arm.
 */
function captainChair({ k, s, ctx }: Recipe) {
  const C = CAPTAIN;
  const leather = s.tinted(shade(ctx.look.palette.metal, 0.5));
  const w = C.seatHalf;
  const [r0, r1] = C.arm;
  k.cylinder(0, 0, 0, C.footHeight, C.foot, 12, s.dark);
  k.cylinder(0, 0, C.footHeight, C.seat - 0.12, C.column, 10, s.metal);
  chair(k, w, C.seat, C.back, leather);
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
 * Round table: a lathe-turned table on a flared foot, a rim ring round
 * its edge and a glowing disc set into the middle of the top.
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
  k.ring(0, 0, top - 0.03, r - 0.06, 0.03, 6, T.sides, s.metal, "up");
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

/** The council chair: seat half width and height, back, leg side. */
const COUNCIL_CHAIR = {
  seatHalf: 0.24,
  seat: 0.46,
  back: 0.95,
  leg: 0.04,
} as const;

/** Council chair: a plain chair on four legs. */
function councilChair({ k, s, ctx }: Recipe) {
  const { seatHalf: w, seat, back, leg } = COUNCIL_CHAIR;
  for (const a of [-w, w - leg]) {
    for (const d of [-w, w - leg]) {
      k.box(a, a + leg, d, d + leg, 0, seat - 0.08, s.metal);
    }
  }
  chair(k, w, seat, back, s.tinted(shade(ctx.look.palette.machine, 0.85)));
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
 * Generator: a heavy block with cooling fins down both sides, a caged
 * glowing core on top and a gauge panel on the front.
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
    s.dark,
    "up",
  );
  k.cylinder(0, 0, coreTop, coreTop + 0.08, G.coreRadius + 0.07, 12, s.metal);
  // The gauge panel on the front face.
  k.bevelBox(-0.4, 0.4, hd - 0.1, hd - 0.05, 0.8, 1.2, 0.01, s.dark);
  for (const a of [-0.25, 0, 0.25]) {
    k.box(a - 0.07, a + 0.07, hd - 0.05, hd - 0.045, 0.95, 1.08, s.glow(core));
  }
}

/** The pipes of a run: depth of each axis and its radius. */
const PIPES: readonly (readonly [d: number, r: number])[] = [
  [-0.18, 0.07],
  [0, 0.05],
  [0.18, 0.07],
];
/** The spacing of flanged joints and brackets, and the brackets' reach. */
const PIPE_JOINT = 1.5;
const BRACKET_INSET = 0.3;
const BRACKET_HALF = 0.3;

/**
 * Pipe run: three pipes along the ceiling with flanges at every joint and
 * brackets hanging them from the ceiling, as long as the hall allows.
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
  const w = BRACKET_HALF;
  for (let i = 0; i <= brackets; i++) {
    const a = -half + BRACKET_INSET + (i * span) / brackets;
    k.box(a - 0.03, a + 0.03, -w, w, h - 0.1, h - 0.07, s.dark);
    k.box(
      a - 0.02,
      a + 0.02,
      -0.02,
      0.02,
      h - 0.07,
      ctx.ceiling - HEADROOM,
      s.dark,
    );
    k.box(a - 0.03, a + 0.03, -w, -w + 0.03, h - 0.1, h + 0.1, s.dark);
    k.box(a - 0.03, a + 0.03, w - 0.03, w, h - 0.1, h + 0.1, s.dark);
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
 * Shelf row: a two-sided run of shelves, end panels and a spine down the
 * middle, with boxes of files on both faces of every shelf, their widths
 * and heights from the seed.
 */
function shelfRow({ k, s, decor }: Recipe) {
  const [hw, hd] = halves("shelf-row");
  const rng = createRng(decor.seed);
  const top = SHELF.top;
  for (const a of [-hw, hw - 0.05])
    k.bevelBox(a, a + 0.05, -hd, hd, 0, top, 0.01, s.body);
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
 * Lab island: a bench island with a sink and its tap at one end and a
 * fume hood with a glowing work light and an exhaust duct at the other.
 */
function labIsland({ k, s, ctx }: Recipe) {
  const L = LAB_ISLAND;
  const [hw, hd] = halves("lab-island");
  const top = L.top;
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
  // The sink: a raised steel rim, the dark basin and the tap.
  const [s0, s1] = L.sink;
  const tap = (s0 + s1) / 2;
  k.bevelBox(s0, s1, 0.05, 0.55, top, top + 0.03, 0.01, s.metal);
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
 * Specimen tank: a glass tank glowing with the liquid it holds, on a base,
 * with a cap, a collar ring and feed tubes on top.
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
  k.ring(0, 0, T.collar, g, 0.03, 6, T.sides, s.metal, "up");
  k.cylinder(0, 0, T.glass, T.cap, r, T.sides, s.body);
  for (const a of [-0.15, 0.15]) {
    k.cylinder(a, 0, T.cap, T.cap + T.tubes, 0.025, 6, s.metal);
  }
}

/** The recipe of every decor kind. */
const RECIPES: Record<DecorKind, (r: Recipe) => void> = {
  "command-console": commandConsole,
  "captain-chair": captainChair,
  "round-table": roundTable,
  "council-chair": councilChair,
  generator,
  "pipe-run": pipeRun,
  "shelf-row": shelfRow,
  "lab-island": labIsland,
  "specimen-tank": specimenTank,
};
