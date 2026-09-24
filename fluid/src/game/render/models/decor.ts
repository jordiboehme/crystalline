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
const CONSOLE_DEPTH = 0.5;

/**
 * Command console: an arc of three console segments, the middle one
 * square to the front and the two wings swung forward, each with a
 * sloped deck of toggles and a screen housing with its glowing screen.
 */
function commandConsole({ kitAt, f, s, ctx }: Recipe) {
  const [, hd] = halves("command-console");
  const back = offset(f, 0, -hd + 0.2);
  const screen = s.glow(shade(ctx.look.palette.screenText, 0.7));
  const segment = (sf: Frame, a0: number, a1: number) => {
    const k = kitAt(sf);
    profileAlong(
      kitAt,
      sf,
      [
        [0, 0],
        [CONSOLE_DEPTH, 0],
        [CONSOLE_DEPTH, 0.7],
        [0.2, 0.92],
        [0.12, 0.92],
        [0.12, 1.25],
        [0, 1.25],
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
    k.panel(a0 + 0.08, a1 - 0.08, 0.121, 0.97, 1.2, screen);
    // Two rows of toggles up the deck.
    for (let row = 0; row < 2; row++) {
      const d = CONSOLE_DEPTH - 0.08 - row * 0.12;
      const h = 0.7 + ((CONSOLE_DEPTH - d) / (CONSOLE_DEPTH - 0.2)) * 0.22;
      for (let a = a0 + 0.1; a + 0.05 < a1 - 0.05; a += 0.1) {
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

/**
 * Captain's chair: a high-backed chair on a pedestal, with armrests and a
 * small glowing control pad on the right arm.
 */
function captainChair({ k, s, ctx }: Recipe) {
  const leather = s.tinted(shade(ctx.look.palette.metal, 0.5));
  k.cylinder(0, 0, 0, 0.08, 0.3, 12, s.dark);
  k.cylinder(0, 0, 0.08, 0.4, 0.07, 10, s.metal);
  chair(k, 0.3, 0.52, 1.3, leather);
  for (const dir of [-1, 1]) {
    const [a0, a1] = dir < 0 ? [-0.38, -0.3] : [0.3, 0.38];
    k.box(a0 + 0.02, a1 - 0.02, 0.05, 0.1, 0.52, 0.64, s.metal);
    k.bevelBox(a0, a1, -0.25, 0.25, 0.64, 0.7, 0.02, leather);
  }
  k.box(0.31, 0.37, 0.1, 0.2, 0.7, 0.71, s.glow(ctx.look.palette.door));
}

/**
 * Round table: a lathe-turned table on a flared foot, a rim ring round
 * its edge and a glowing disc set into the middle of the top.
 */
function roundTable({ k, s, ctx }: Recipe) {
  const [r] = halves("round-table");
  const top = 0.78;
  k.lathe(
    0,
    0,
    [
      [0, 0],
      [0.5, 0],
      [0.5, 0.05],
      [0.18, 0.12],
      [0.18, 0.64],
      [r - 0.1, top - 0.06],
      [r - 0.06, top],
      [0.42, top],
      [0, top],
    ],
    24,
    s.body,
  );
  k.ring(0, 0, top - 0.03, r - 0.06, 0.03, 6, 24, s.metal, "up");
  k.cylinder(0, 0, top, top + 0.01, 0.4, 24, s.glow(ctx.look.palette.door));
}

/** Council chair: a plain chair on four legs. */
function councilChair({ k, s, ctx }: Recipe) {
  const w = 0.24;
  for (const a of [-w, w - 0.04]) {
    for (const d of [-w, w - 0.04])
      k.box(a, a + 0.04, d, d + 0.04, 0, 0.38, s.metal);
  }
  chair(k, w, 0.46, 0.95, s.tinted(shade(ctx.look.palette.machine, 0.85)));
}

/**
 * Generator: a heavy block with cooling fins down both sides, a caged
 * glowing core on top and a gauge panel on the front.
 */
function generator({ k, s, decor }: Recipe) {
  const [hw, hd] = halves("generator");
  const body = hw - 0.3;
  k.bevelBox(-body, body, -hd + 0.1, hd - 0.1, 0, 1.6, 0.05, s.body);
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
    for (let i = 0; i < 8; i++) {
      const d = -hd + 0.3 + i * ((2 * hd - 0.6) / 7);
      const [a0, a1] = dir < 0 ? [-hw + 0.08, -body] : [body, hw - 0.08];
      k.box(a0, a1, d - 0.02, d + 0.02, 0.2, 1.4, s.metal);
    }
  }
  const core = hueToRgb(createRng(decor.seed).range(170, 210), 0.9, 0.55);
  k.cylinder(0, 0, 1.6, 1.9, 0.25, 12, s.glow(core));
  k.ring(0, 0, 1.75, 0.27, 0.03, 6, 12, s.dark, "up");
  k.cylinder(0, 0, 1.9, 1.98, 0.32, 12, s.metal);
  // The gauge panel on the front face.
  k.bevelBox(-0.4, 0.4, hd - 0.1, hd - 0.05, 0.8, 1.2, 0.01, s.dark);
  for (const a of [-0.25, 0, 0.25]) {
    k.box(a - 0.07, a + 0.07, hd - 0.05, hd - 0.045, 0.95, 1.08, s.glow(core));
  }
}

/**
 * Pipe run: three pipes along the ceiling with flanges at every joint and
 * brackets hanging them from the ceiling, as long as the hall allows.
 */
function pipeRun({ k, s, ctx, decor }: Recipe) {
  const half = pipeLength(decor, ctx.hall) / 2;
  if (half <= 0) return;
  const h = ctx.ceiling - PIPE_DROP;
  const pipes: [number, number][] = [
    [-0.18, 0.07],
    [0, 0.05],
    [0.18, 0.07],
  ];
  for (const [d, r] of pipes) {
    k.cylinderAlong(-half, half, d, h, r, 8, s.metal);
    for (let a = -half + 1; a < half - 0.5; a += 1.5) {
      k.cylinderAlong(a - 0.04, a + 0.04, d, h, r + 0.02, 8, s.dark);
    }
  }
  const span = Math.max(0, 2 * half - 0.6);
  const brackets = Math.max(1, Math.round(span / 1.5));
  for (let i = 0; i <= brackets; i++) {
    const a = -half + 0.3 + (i * span) / brackets;
    k.box(a - 0.03, a + 0.03, -0.3, 0.3, h - 0.1, h - 0.07, s.dark);
    k.box(
      a - 0.02,
      a + 0.02,
      -0.02,
      0.02,
      h - 0.07,
      ctx.ceiling - HEADROOM,
      s.dark,
    );
    k.box(a - 0.03, a + 0.03, -0.3, -0.27, h - 0.1, h + 0.1, s.dark);
    k.box(a - 0.03, a + 0.03, 0.27, 0.3, h - 0.1, h + 0.1, s.dark);
  }
}

/**
 * Shelf row: a two-sided run of shelves, end panels and a spine down the
 * middle, with boxes of files on both faces of every shelf, their widths
 * and heights from the seed.
 */
function shelfRow({ k, s, decor }: Recipe) {
  const [hw, hd] = halves("shelf-row");
  const rng = createRng(decor.seed);
  const top = 2.2;
  for (const a of [-hw, hw - 0.05])
    k.bevelBox(a, a + 0.05, -hd, hd, 0, top, 0.01, s.body);
  k.box(-0.025, 0.025, -hd + 0.02, hd - 0.02, 0, top, s.body);
  k.box(-hw + 0.05, hw - 0.05, -0.01, 0.01, 0.05, top, s.dark);
  k.box(-hw, hw, -hd, hd, top, top + 0.04, s.metal);
  const levels = [0.05, 0.6, 1.15, 1.7];
  const colours = [0, 30, 55, 200, 220].map((hue) =>
    s.tinted(hueToRgb(hue, 0.25, 0.45)),
  );
  for (const h of levels) {
    k.box(-hw + 0.05, hw - 0.05, -hd + 0.02, hd - 0.02, h, h + 0.03, s.metal);
    for (const side of [-1, 1]) {
      for (const [a0, a1] of [
        [-hw + 0.08, -0.05],
        [0.05, hw - 0.08],
      ] as const) {
        let a = a0;
        while (a < a1 - 0.12) {
          const w = Math.min(rng.range(0.14, 0.34), a1 - a);
          const height = rng.range(0.22, 0.44);
          if (rng.next() > 0.12) {
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
 * Lab island: a bench island with a sink and its tap at one end and a
 * fume hood with a glowing work light and an exhaust duct at the other.
 */
function labIsland({ k, s, ctx }: Recipe) {
  const [hw, hd] = halves("lab-island");
  const top = 0.96;
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
  k.bevelBox(0.55, 1.15, 0.05, 0.55, top, top + 0.03, 0.01, s.metal);
  k.box(0.6, 1.1, 0.1, 0.5, top + 0.03, top + 0.032, s.dark);
  k.cylinder(0.85, 0.0, top, top + 0.3, 0.02, 8, s.metal);
  k.box(0.83, 0.87, 0.0, 0.2, top + 0.27, top + 0.3, s.metal);
  // The fume hood: back, sides, top, the glowing work light and the duct.
  const [h0, h1] = [-hw + 0.1, -0.2];
  const hoodTop = 1.9;
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
    Math.min(2.4, ctx.ceiling - HEADROOM),
    0.12,
    10,
    s.metal,
  );
}

/**
 * Specimen tank: a glass tank glowing with the liquid it holds, on a base,
 * with a cap, a collar ring and feed tubes on top.
 */
function specimenTank({ k, s, decor }: Recipe) {
  const [hw] = halves("specimen-tank");
  const r = hw - 0.03;
  k.cylinder(0, 0, 0, 0.25, r, 16, s.body);
  const liquid = hueToRgb(createRng(decor.seed).range(90, 180), 0.7, 0.5);
  k.lathe(
    0,
    0,
    [
      [0, 0.25],
      [r - 0.08, 0.25],
      [r - 0.04, 0.5],
      [r - 0.04, 1.4],
      [r - 0.08, 1.6],
      [0, 1.6],
    ],
    16,
    s.glow(liquid),
  );
  k.ring(0, 0, 0.95, r - 0.04, 0.03, 6, 16, s.metal, "up");
  k.cylinder(0, 0, 1.6, 1.8, r, 16, s.body);
  for (const a of [-0.15, 0.15]) k.cylinder(a, 0, 1.8, 2.0, 0.025, 6, s.metal);
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
