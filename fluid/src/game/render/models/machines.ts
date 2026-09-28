/**
 * The machines: one per tag, the kind picked by the tag's hash, so the
 * same tag looks the same in every room.
 *
 * Every machine stands against its wall slot inside its `FOOTPRINTS` size,
 * with the tag strip and the tag's label on the wall above it. Each recipe
 * below says what it depicts. Every part that glows (a lamp, LEDs, a core,
 * a grow light) sits on or in the machine's body, never in mid-air.
 *
 * A kind draws one of its variants (`RECIPES`, 2.7 C2), the tag's own
 * (`machineVariant`): variant 0 is the kind's first model, part for part
 * (2.7 C1), and every other variant is another machine of the same kind
 * that keeps the kind's envelope (2.7 C3): its footprint, and every curio
 * top and under spot of `FIXTURE_SURFACES.machine` with the clear height
 * over it. Every variant bakes the tag's second accent (2.7 C10,
 * `look.accents[tagAccent(tag)]`) into one trim part, which on variant 0 is
 * a part it already has, re-tinted and nothing more.
 */

import { createRng, type Rng } from "../../core/seed";
import { FOOTPRINTS } from "../../world/footprints";
import type { Fixture, MachineKind } from "../../world/types";
import { machineModelSeed, tagAccent } from "../../world/variants";
import type { Surface } from "../geometry";
import { DECAL_LIFT, frameForSlot, type Frame, type Kit } from "../kit";
import { LAYER } from "../layers";
import { hueToRgb, type Rgb } from "../looks";
import {
  discOutline,
  profileAlong,
  shade,
  sideways,
  surfaces,
  tiltedBar,
  type KitAt,
  type ModelContext,
  type Surfaces,
} from "./common";
import { tagStrip } from "./wall";

type Machine = Extract<Fixture, { kind: "machine" }>;

/**
 * What a recipe gets: its kits, the look's surfaces, the tag's colour, the
 * tag's second accent (`accent2`, 2.7 C10: `look.accents[tagAccent(tag)]`,
 * baked into one trim part of every variant). Each variant is its own
 * recipe, so none is told which variant it draws.
 */
interface Recipe {
  k: Kit;
  kitAt: KitAt;
  f: Frame;
  s: Surfaces;
  ctx: ModelContext;
  hue: Rgb;
  rng: Rng;
  /** Half the footprint along the wall, and how far out it reaches. */
  half: number;
  out: number;
  accent2: Rgb;
}

/**
 * A surface re-tinted in the tag's second accent (2.7 C10): the part keeps
 * its own layer and flag, so on a variant 0 the accent changes a trim
 * part's colour and nothing else (2.7 C1).
 */
const trim = (s: Surface, accent2: Rgb): Surface => ({ ...s, tint: accent2 });

/**
 * Builds a machine and its tag strip against its wall slot. Its random
 * details are drawn from `machineModelSeed(fx.tag)`, the tag alone, never
 * the fixture's own `seed`, so the same tag is the same machine, part for
 * part, in every room (2.7 C5, Review Focus 2). Throws on a variant the
 * kind has no recipe for.
 */
export function buildMachine(
  kitAt: KitAt,
  fx: Machine,
  index: number,
  ctx: ModelContext,
): void {
  const f = frameForSlot(fx.slot);
  const k = kitAt(f);
  const size = FOOTPRINTS.machine[fx.machine];
  const hue = hueToRgb(fx.hue, 0.85, 0.55);
  const recipes = RECIPES[fx.machine];
  const variant = fx.variant ?? 0;
  const recipe = recipes[variant];
  if (recipe === undefined)
    throw new Error(`machine ${fx.machine}: no variant ${String(variant)}`);
  recipe({
    k,
    kitAt,
    f,
    s: surfaces(ctx.look),
    ctx,
    hue,
    rng: createRng(machineModelSeed(fx.tag)),
    half: size.along / 2,
    out: size.out,
    accent2: ctx.look.accents[tagAccent(fx.tag)] ?? ctx.look.palette.metal,
  });
  tagStrip(k, ctx, `tag:${index}`, hue);
}

/** The side of a square table leg. */
const LEG = 0.05;

/** Four square legs under a top spanning `a0..a1`, `d0..d1`, up to `h`. */
function legs(
  k: Kit,
  a0: number,
  a1: number,
  d0: number,
  d1: number,
  h: number,
  s: Surface,
) {
  for (const a of [a0, a1 - LEG]) {
    for (const d of [d0, d1 - LEG]) k.box(a, a + LEG, d, d + LEG, 0, h, s);
  }
}

/** The workbench: top height and thickness, shelf height, pegboard, lamp. */
const WORKBENCH = {
  top: 0.9,
  slab: 0.06,
  shelf: 0.18,
  vice: 0.2,
  pegboard: [1.1, 1.8],
  hook: 1.7,
  tools: 6,
  lampPost: 1.45,
  lampReach: 0.5,
  shade: [1.3, 1.44],
  shadeRadius: 0.1,
} as const;

/**
 * Workbench: a steel bench on legs with a lower shelf, a vice clamped to
 * its front corner, a pegboard tool rack on the wall with tools hanging
 * from it, and a lamp on an arm whose bulb glows under its shade.
 */
function workbench({ k, s, ctx, rng, half, out, accent2 }: Recipe) {
  const W = WORKBENCH;
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const top = W.top;
  const front = out - 0.1;
  k.bevelBox(a0, a1, 0.02, front, top - W.slab, top, 0.012, s.body);
  legs(k, a0 + 0.04, a1 - 0.04, 0.05, front - 0.04, top - W.slab, s.metal);
  k.box(
    a0 + 0.06,
    a1 - 0.06,
    0.08,
    front - 0.06,
    W.shelf,
    W.shelf + 0.04,
    s.metal,
  );
  // The vice: base, fixed jaw, moving jaw and the screw's tommy bar.
  const v = a1 - 0.3;
  const [j0, j1] = [top + 0.05, top + 0.13];
  k.bevelBox(v, v + W.vice, out - 0.3, front - 0.02, top, j0, 0.01, s.dark);
  const jaw = trim(s.dark, accent2);
  k.box(v + 0.03, v + W.vice - 0.03, out - 0.3, out - 0.25, j0, j1, jaw);
  k.box(v + 0.03, v + W.vice - 0.03, out - 0.19, out - 0.14, j0, j1, jaw);
  k.cylinderAlong(
    v - 0.02,
    v + W.vice + 0.02,
    front - 0.02,
    top + 0.09,
    0.01,
    6,
    s.metal,
  );
  // The pegboard and its tools, their lengths from the seed.
  const [p0, p1] = W.pegboard;
  k.bevelBox(
    a0 + 0.05,
    0.35,
    0,
    0.03,
    p0,
    p1,
    0.01,
    s.tinted(shade(ctx.look.palette.metal, 0.7), LAYER.metal),
  );
  for (let i = 0; i < W.tools; i++) {
    const a = a0 + 0.15 + i * 0.17;
    const len = rng.range(0.18, 0.4);
    k.box(a, a + 0.03, 0.03, 0.05, W.hook - len, W.hook, s.metal);
    k.box(
      a - 0.02,
      a + 0.05,
      0.03,
      0.055,
      W.hook - 0.04,
      W.hook + 0.02,
      s.dark,
    );
  }
  // The lamp: foot, post, arm, shade and the glowing bulb under it.
  const [la, ld] = [a1 - 0.12, 0.14];
  const [s0, s1] = W.shade;
  k.cylinder(la, ld, top, top + 0.03, 0.07, 10, s.dark);
  k.box(
    la - 0.015,
    la + 0.015,
    ld - 0.015,
    ld + 0.015,
    top + 0.03,
    W.lampPost,
    s.metal,
  );
  k.box(
    la - 0.015,
    la + 0.015,
    ld - 0.015,
    W.lampReach,
    W.lampPost - 0.03,
    W.lampPost,
    s.metal,
  );
  k.lathe(
    la,
    W.lampReach,
    [
      [0, s0],
      [W.shadeRadius, s0],
      [0.04, s1 - 0.02],
      [0, s1],
    ],
    10,
    s.dark,
  );
  k.cylinder(
    la,
    W.lampReach,
    s0 - 0.015,
    s0,
    W.shadeRadius - 0.025,
    10,
    s.glow(ctx.look.palette.lamp),
  );
}

/** The lab bench: top height, flask spots, the rack and the reagent shelf. */
const LAB_BENCH = {
  top: 0.94,
  resin: 0.05,
  flaskDepth: 0.45,
  conical: -0.45,
  round: -0.1,
  rack: [0.25, 0.65],
  rackDepth: [0.3, 0.42],
  tubes: 6,
  shelf: 1.5,
  bottles: 7,
} as const;

/**
 * Lab bench: a cabinet bench with a resin top, two glass flasks glowing
 * with what they hold, a rack of test tubes and a shelf of reagent
 * bottles on the wall.
 */
function labBench({ k, s, ctx, hue, rng, half, out, accent2 }: Recipe) {
  const L = LAB_BENCH;
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const top = L.top;
  const cabinet = out - 0.18;
  k.bevelBox(
    a0 + 0.02,
    a1 - 0.02,
    0.02,
    cabinet,
    0,
    top - L.resin,
    0.02,
    s.body,
  );
  for (let i = 0; i < 3; i++) {
    const c = a0 + 0.1 + i * ((a1 - a0 - 0.2) / 3);
    k.box(c, c + 0.5, cabinet, cabinet + 0.01, 0.1, top - 0.15, s.panel);
    k.box(
      c + 0.2,
      c + 0.3,
      cabinet + 0.01,
      cabinet + 0.03,
      top - 0.25,
      top - 0.22,
      trim(s.dark, accent2),
    );
  }
  k.bevelBox(
    a0,
    a1,
    0,
    out - 0.12,
    top - L.resin,
    top,
    0.01,
    s.tinted(shade(ctx.look.palette.metal, 0.4)),
  );
  // Two flasks: a conical one and a round-bottomed one on a ring stand.
  const liquid = s.glow(hueToRgb(rng.range(0, 360), 0.8, 0.6));
  k.lathe(
    L.conical,
    L.flaskDepth,
    [
      [0, top],
      [0.1, top],
      [0.1, top + 0.03],
      [0.03, top + 0.18],
      [0.03, top + 0.26],
      [0, top + 0.26],
    ],
    10,
    liquid,
  );
  k.cylinder(L.round, L.flaskDepth, top, top + 0.12, 0.05, 8, s.dark);
  k.lathe(
    L.round,
    L.flaskDepth,
    [
      [0, top + 0.1],
      [0.06, top + 0.12],
      [0.085, top + 0.18],
      [0.06, top + 0.24],
      [0.02, top + 0.26],
      [0.02, top + 0.34],
      [0, top + 0.34],
    ],
    10,
    s.glow(hue),
  );
  // The test tube rack: base, top bar, end posts and the tubes.
  const [r0, r1] = L.rack;
  const [rd0, rd1] = L.rackDepth;
  k.box(r0, r1, rd0, rd1, top, top + 0.02, s.dark);
  k.box(r0, r1, rd0, rd1, top + 0.1, top + 0.12, s.dark);
  k.box(r0, r0 + 0.02, rd0, rd1, top, top + 0.12, s.dark);
  k.box(r1 - 0.02, r1, rd0, rd1, top, top + 0.12, s.dark);
  for (let i = 0; i < L.tubes; i++) {
    const a = r0 + 0.06 + i * 0.056;
    k.cylinder(
      a,
      (rd0 + rd1) / 2,
      top + 0.02,
      top + rng.range(0.16, 0.2),
      0.012,
      6,
      s.tinted(shade(hue, 0.9)),
    );
  }
  // The reagent shelf and its bottles.
  k.box(a0 + 0.1, a1 - 0.1, 0, 0.22, L.shelf, L.shelf + 0.03, s.metal);
  for (let i = 0; i < L.bottles; i++) {
    const a = a0 + 0.2 + i * 0.2;
    k.cylinder(
      a,
      0.12,
      L.shelf + 0.03,
      L.shelf + 0.03 + rng.range(0.12, 0.22),
      0.04,
      8,
      s.tinted(hueToRgb(rng.range(0, 360), 0.5, 0.4)),
    );
  }
}

/** The server rack: cabinet height, the unit slabs and their pitch. */
const SERVER_RACK = {
  top: 2.2,
  units: 10,
  firstUnit: 0.14,
  pitch: 0.19,
  unit: 0.16,
  leds: 3,
  vents: 5,
} as const;

/**
 * Server rack: a tall cabinet with ten unit slabs, each with a row of LED
 * dots, a vented top and a cable duct to the wall.
 */
function serverRack({ k, s, ctx, hue, rng, half, out, accent2 }: Recipe) {
  const R = SERVER_RACK;
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const [d1, top] = [out - 0.05, R.top];
  k.bevelBox(a0, a1, 0.02, d1, 0, top, 0.025, s.body);
  k.box(-0.15, 0.15, 0, 0.02, top - 0.3, top - 0.1, s.dark);
  const leds: Rgb[] = [
    hue,
    ctx.look.palette.screenText,
    hueToRgb(35, 0.9, 0.55),
  ];
  for (let u = 0; u < R.units; u++) {
    const h0 = R.firstUnit + u * R.pitch;
    k.bevelBox(
      a0 + 0.04,
      a1 - 0.04,
      d1,
      d1 + 0.02,
      h0,
      h0 + R.unit,
      0.008,
      s.dark,
    );
    k.box(
      a0 + 0.06,
      a1 - 0.25,
      d1 + 0.02,
      d1 + 0.024,
      h0 + 0.04,
      h0 + 0.12,
      s.metal,
    );
    for (let i = 0; i < R.leds; i++) {
      const a = a1 - 0.2 + i * 0.05;
      const c = leds[rng.int(0, leds.length - 1)] ?? hue;
      k.box(
        a,
        a + 0.02,
        d1 + 0.02,
        d1 + 0.026,
        h0 + 0.07,
        h0 + 0.09,
        s.glow(c),
      );
    }
  }
  for (let i = 0; i < R.vents; i++) {
    const a = a0 + 0.1 + i * 0.12;
    k.box(a, a + 0.06, 0.1, d1 - 0.1, top, top + 0.02, trim(s.dark, accent2));
  }
}

/** The cryo pod: plinth, capsule profile heights, window. */
const CRYO_POD = {
  radius: 0.42,
  plinth: 0.15,
  shoulder: 0.3,
  crown: 1.75,
  top: 2.07,
  window: [0.45, 1.75],
  windowHalf: 0.28,
  sides: 14,
} as const;

/**
 * Cryo pod: an upright capsule on a round plinth with a collar ring, a
 * framed glass front glowing faintly with the cold inside, and feed lines
 * to the wall.
 */
function cryoPod({ k, s, ctx, out, accent2 }: Recipe) {
  const C = CRYO_POD;
  const d = out / 2;
  const r = Math.min(C.radius, d - 0.06);
  k.cylinder(0, d, 0, C.plinth, r + 0.03, C.sides, s.dark);
  k.ring(0, d, C.plinth + 0.01, r, 0.04, 6, 16, trim(s.metal, accent2), "up");
  k.lathe(
    0,
    d,
    [
      [0, C.plinth],
      [r - 0.06, C.plinth],
      [r, C.shoulder],
      [r, C.crown],
      [r - 0.08, C.crown + 0.2],
      [0.15, C.top - 0.02],
      [0, C.top],
    ],
    C.sides,
    s.body,
  );
  // The window frame and the glass in it.
  const front = d + r * Math.cos(Math.PI / C.sides) - 0.02;
  const [w0, w1] = C.window;
  k.bevelBox(
    -C.windowHalf,
    C.windowHalf,
    front - 0.12,
    front + 0.03,
    w0,
    w1,
    0.02,
    s.metal,
  );
  k.panel(
    -C.windowHalf + 0.06,
    C.windowHalf - 0.06,
    front + 0.03 + DECAL_LIFT,
    w0 + 0.07,
    w1 - 0.07,
    s.glow(shade(ctx.look.palette.portalAlt, 0.55)),
  );
  // Feed lines and a control box.
  for (const a of [-0.3, 0.3])
    k.box(a - 0.03, a + 0.03, 0, d - r + 0.1, 0.3, 0.36, s.dark);
  k.bevelBox(-0.12, 0.12, 0, 0.06, 1.2, 1.5, 0.01, s.dark);
  k.box(
    -0.06,
    0.06,
    0.06,
    0.065,
    1.35,
    1.42,
    s.glow(ctx.look.palette.screenText),
  );
}

/** The fabricator: chamber floor and ceiling, total height, side block width. */
const FABRICATOR = {
  chamber: [0.6, 1.3],
  top: 1.6,
  side: 0.3,
  back: 0.35,
  rail: 0.6,
} as const;

/**
 * Fabricator: a squat box with an inner chamber open at the front, lit
 * from the top inside, a print head on a nozzle arm running on a rail,
 * a half printed part on the bed and a control screen on the side.
 */
function fabricator({ k, s, hue, half, out, accent2 }: Recipe) {
  const F = FABRICATOR;
  const [a0, a1] = [-half + 0.05, half - 0.05];
  const d1 = out - 0.05;
  const [c0, c1] = F.chamber;
  const [side0, side1] = [a0 + F.side, a1 - F.side];
  k.bevelBox(a0, a1, 0.02, d1, 0, c0, 0.03, s.body);
  k.bevelBox(a0, a1, 0.02, d1, c1, F.top, 0.03, s.body);
  k.box(a0, side0, 0.02, d1, c0, c1, s.body);
  k.box(side1, a1, 0.02, d1, c0, c1, s.body);
  k.box(side0, side1, 0.02, F.back, c0, c1, s.dark);
  // The bed, the glowing chamber light and the printed part.
  k.box(side0 + 0.05, side1 - 0.05, F.back, d1 - 0.05, c0, c0 + 0.03, s.metal);
  k.box(side0 + 0.02, side1 - 0.02, 0.4, 0.5, c1 - 0.02, c1, s.glow(hue));
  k.bevelBox(
    -0.12,
    0.08,
    0.5,
    0.65,
    c0 + 0.03,
    c0 + 0.15,
    0.02,
    s.tinted(shade(hue, 0.7)),
  );
  // The rail, the carriage and its nozzle.
  k.cylinderAlong(side0, side1, F.rail, c1 - 0.06, 0.02, 8, s.metal);
  k.box(
    -0.06,
    0.06,
    F.rail - 0.05,
    F.rail + 0.05,
    c1 - 0.12,
    c1 - 0.03,
    trim(s.dark, accent2),
  );
  k.cylinder(0, F.rail, c0 + 0.25, c1 - 0.12, 0.025, 8, s.metal);
  k.cylinder(0, F.rail, c0 + 0.18, c0 + 0.25, 0.012, 6, s.dark);
  // A control screen on the right side block.
  k.panel(
    side1 + 0.04,
    a1 - 0.04,
    d1 + DECAL_LIFT,
    0.95,
    1.2,
    s.glow(shade(hue, 0.8)),
  );
}

/** Hydroponics: the trough's underside and rim, the domes, the grow light. */
const HYDROPONICS = {
  trough: [0.55, 0.8],
  halfDepth: 0.3,
  domes: [-0.55, 0, 0.55],
  domeRadius: 0.25,
  domeHeight: 0.25,
  light: 1.5,
} as const;

/**
 * Hydroponics: a long trough on legs with three domes of foliage growing
 * out of it and a grow-light bar glowing magenta on arms from the wall.
 */
function hydroponics({ k, s, half, out, accent2 }: Recipe) {
  const H = HYDROPONICS;
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const [t0, t1] = H.trough;
  const dc = out / 2 - 0.05;
  const hd = H.halfDepth;
  k.bevelBox(a0, a1, dc - hd, dc + hd, t0, t1, 0.02, s.body);
  legs(k, a0 + 0.06, a1 - 0.06, dc - hd + 0.04, dc + hd - 0.04, t0, s.metal);
  k.box(
    a0 + 0.04,
    a1 - 0.04,
    dc - hd + 0.04,
    dc + hd - 0.04,
    t1,
    t1 + 0.01,
    s.tinted(hueToRgb(30, 0.4, 0.2)),
  );
  const leaf = s.tinted(hueToRgb(115, 0.5, 0.35));
  const [r, h] = [H.domeRadius, H.domeHeight];
  for (const a of H.domes) {
    k.lathe(
      a,
      dc,
      [
        [0, t1],
        [r, t1],
        [r - 0.01, t1 + 0.32 * h],
        [0.8 * r, t1 + 0.68 * h],
        [0.48 * r, t1 + 0.92 * h],
        [0, t1 + h],
      ],
      10,
      leaf,
    );
  }
  // The grow-light bar on two arms.
  for (const a of [a0 + 0.1, a1 - 0.14])
    k.box(
      a,
      a + 0.04,
      0,
      dc + 0.05,
      H.light,
      H.light + 0.04,
      trim(s.metal, accent2),
    );
  k.box(
    a0 + 0.05,
    a1 - 0.05,
    dc - 0.05,
    dc + 0.05,
    H.light - 0.04,
    H.light,
    s.glow(hueToRgb(300, 0.8, 0.6)),
  );
}

/**
 * The bench every workbench variant shares: the steel top, its legs and
 * the lower shelf, the same as variant 0's, so both curio spots (the top
 * and the shelf under it) stay where they are (2.7 C3).
 */
function workbenchFrame(
  k: Kit,
  s: Surfaces,
  a0: number,
  a1: number,
  front: number,
) {
  const W = WORKBENCH;
  const top = W.top;
  k.bevelBox(a0, a1, 0.02, front, top - W.slab, top, 0.012, s.body);
  legs(k, a0 + 0.04, a1 - 0.04, 0.05, front - 0.04, top - W.slab, s.metal);
  k.box(
    a0 + 0.06,
    a1 - 0.06,
    0.08,
    front - 0.06,
    W.shelf,
    W.shelf + 0.04,
    s.metal,
  );
}

/** The fitter's bench: the drawer unit, the wall cabinet and the ring lamp. */
const FITTER = {
  drawers: { a: [0.36, 0.78], d: [0.705, 0.79], h: [0.58, 0.84] },
  cabinet: { a: [0.5, 1.0], d: 0.27, h: [1.2, 1.85] },
  lamp: { a: 0.7, post: 0.4, centre: 0.5, h: 1.27, radius: 0.1 },
} as const;

/**
 * Workbench variant 1, a fitter's bench: the same bench and lower shelf, a
 * three-drawer unit hung under the top's right end in front of the shelf
 * spot, a closed two-door cabinet on the wall over the right end, and a
 * ring lamp on a short post, its lens glowing under the ring. No pegboard
 * and no vice. The drawer pulls carry the tag's second accent.
 */
function fitterBench({ k, s, ctx, half, out, accent2 }: Recipe) {
  const W = WORKBENCH;
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const top = W.top;
  const front = out - 0.1;
  workbenchFrame(k, s, a0, a1, front);
  // The drawer unit: its body, three fronts and their pulls.
  const D = FITTER.drawers;
  const [da0, da1] = D.a;
  const [dd0, dd1] = D.d;
  const [dh0, dh1] = D.h;
  k.box(da0, da1, dd0, dd1, dh0, dh1, s.body);
  const pitch = (dh1 - dh0) / 3;
  for (let i = 0; i < 3; i++) {
    const h0 = dh0 + i * pitch;
    k.box(
      da0 + 0.01,
      da1 - 0.01,
      dd1,
      dd1 + 0.01,
      h0 + 0.01,
      h0 + pitch - 0.01,
      s.metal,
    );
    const mid = (da0 + da1) / 2;
    k.box(
      mid - 0.06,
      mid + 0.06,
      dd1 + 0.01,
      dd1 + 0.025,
      h0 + pitch / 2 - 0.012,
      h0 + pitch / 2 + 0.012,
      trim(s.dark, accent2),
    );
  }
  // The wall cabinet over the bench's right end, two doors and two knobs.
  const C = FITTER.cabinet;
  const [ca0, ca1] = C.a;
  const [ch0, ch1] = C.h;
  k.bevelBox(ca0, ca1, 0, C.d, ch0, ch1, 0.012, s.body);
  const mid = (ca0 + ca1) / 2;
  for (const [x0, x1] of [
    [ca0 + 0.02, mid - 0.005],
    [mid + 0.005, ca1 - 0.02],
  ] as const) {
    k.box(x0, x1, C.d, C.d + 0.012, ch0 + 0.03, ch1 - 0.03, s.panel);
  }
  for (const x of [mid - 0.04, mid + 0.02])
    k.box(x, x + 0.02, C.d + 0.012, C.d + 0.028, 1.5, 1.58, s.dark);
  // The ring lamp: foot, post, the ring and its glowing lens.
  const L = FITTER.lamp;
  k.cylinder(L.a, L.post, top, top + 0.03, 0.06, 10, s.dark);
  k.box(
    L.a - 0.015,
    L.a + 0.015,
    L.post - 0.015,
    L.post + 0.015,
    top + 0.03,
    L.h,
    s.metal,
  );
  k.ring(L.a, L.centre, L.h, L.radius, 0.02, 6, 14, s.dark, "up");
  k.cylinder(
    L.a,
    L.centre,
    L.h - 0.012,
    L.h + 0.004,
    L.radius - 0.015,
    14,
    s.glow(ctx.look.palette.lamp),
  );
}

/** The welding bench: the gas bottles, the hose coil and the work light. */
const WELDING = {
  lip: 0.035,
  bottles: [-0.68, -0.46],
  bottleDepth: 0.81,
  bottleRadius: 0.085,
  bottle: 0.72,
  coil: { a: -0.3, h: 1.42, radius: 0.13 },
  light: { a: 0.7, arm: 1.78, reach: 0.32, shade: [1.6, 1.74] },
} as const;

/**
 * Workbench variant 2, a welding bench: the same bench and lower shelf, a
 * steel lip round the top (clear of the curio top), two gas bottles
 * standing in front of the bench's left end, short enough to tuck under
 * the top, a coil of hose on a hook on the wall and a hooded work light on
 * a wall bracket over the right end. The bottles' valve caps carry the
 * tag's second accent.
 */
function weldingBench({ k, s, ctx, half, out, accent2 }: Recipe) {
  const W = WORKBENCH;
  const B = WELDING;
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const top = W.top;
  const front = out - 0.1;
  workbenchFrame(k, s, a0, a1, front);
  // The lip: a strip on each edge of the top, outside the curio top.
  const lip = top + B.lip;
  k.box(a0, a1, 0.02, 0.07, top, lip, s.metal);
  k.box(a0, a1, front - 0.015, front, top, lip, s.metal);
  k.box(a0, a0 + 0.035, 0.07, front - 0.015, top, lip, s.metal);
  k.box(a1 - 0.035, a1, 0.07, front - 0.015, top, lip, s.metal);
  // The gas bottles: body, shoulder, valve and its cap.
  const tints = [hueToRgb(210, 0.45, 0.35), hueToRgb(0, 0.5, 0.3)];
  B.bottles.forEach((a, i) => {
    const tint = tints[i % tints.length] ?? s.metal.tint;
    const d = B.bottleDepth;
    const r = B.bottleRadius;
    k.cylinder(a, d, 0, B.bottle, r, 12, s.tinted(tint));
    k.lathe(
      a,
      d,
      [
        [r, B.bottle],
        [0.05, B.bottle + 0.05],
        [0, B.bottle + 0.06],
      ],
      12,
      s.tinted(tint),
    );
    k.cylinder(
      a,
      d,
      B.bottle + 0.05,
      B.bottle + 0.1,
      0.03,
      8,
      trim(s.dark, accent2),
    );
  });
  // The hose coil hanging from a hook on the wall.
  const C = B.coil;
  k.box(
    C.a - 0.02,
    C.a + 0.02,
    0,
    0.06,
    C.h + C.radius,
    C.h + C.radius + 0.05,
    s.dark,
  );
  const hose = s.tinted(hueToRgb(10, 0.3, 0.18));
  k.ring(C.a, 0.035, C.h, C.radius, 0.02, 6, 16, hose, "inward");
  k.ring(
    C.a + 0.02,
    0.045,
    C.h - 0.01,
    C.radius - 0.02,
    0.02,
    6,
    16,
    hose,
    "inward",
  );
  // The hooded work light on its wall bracket.
  const L = B.light;
  const [s0, s1] = L.shade;
  k.box(L.a - 0.05, L.a + 0.05, 0, 0.02, L.arm - 0.08, L.arm + 0.04, s.dark);
  k.box(L.a - 0.015, L.a + 0.015, 0.02, L.reach, L.arm, L.arm + 0.03, s.metal);
  k.lathe(
    L.a,
    L.reach,
    [
      [0, s0],
      [0.12, s0],
      [0.05, s1],
      [0, s1 + 0.04],
    ],
    10,
    s.dark,
  );
  k.cylinder(
    L.a,
    L.reach,
    s0 - 0.012,
    s0,
    0.1,
    10,
    s.glow(ctx.look.palette.lamp),
  );
}

/**
 * The cabinet every lab bench variant shares, as variant 0 draws it: the
 * body, three doors with their handles, and a kick strip along its foot in
 * the tag's second accent. Returns the cabinet's front depth.
 */
function labCabinet(
  k: Kit,
  s: Surfaces,
  a0: number,
  a1: number,
  out: number,
  accent2: Rgb,
) {
  const L = LAB_BENCH;
  const top = L.top;
  const cabinet = out - 0.18;
  k.bevelBox(
    a0 + 0.02,
    a1 - 0.02,
    0.02,
    cabinet,
    0,
    top - L.resin,
    0.02,
    s.body,
  );
  for (let i = 0; i < 3; i++) {
    const c = a0 + 0.1 + i * ((a1 - a0 - 0.2) / 3);
    k.box(c, c + 0.5, cabinet, cabinet + 0.01, 0.1, top - 0.15, s.panel);
    k.box(
      c + 0.2,
      c + 0.3,
      cabinet + 0.01,
      cabinet + 0.03,
      top - 0.25,
      top - 0.22,
      s.dark,
    );
  }
  k.box(
    a0 + 0.04,
    a1 - 0.04,
    cabinet - 0.01,
    cabinet + 0.012,
    0.01,
    0.08,
    trim(s.dark, accent2),
  );
  return cabinet;
}

/** The wet bench: the sink's opening, the tap, the fume arm, the cabinet. */
const WET_BENCH = {
  sink: { a: [-0.35, 0.25], d: [0.3, 0.62] },
  tap: { a: -0.05, d: 0.15, h: 1.2, reach: 0.15 },
  arm: { a: 0.45, h: 1.85, reach: 0.5, hood: [1.3, 1.46] },
  cabinet: { a: [-0.85, 0.25], d: 0.22, h: [1.35, 1.9] },
} as const;

/**
 * Lab bench variant 1, a wet bench: the same cabinet, its resin top cut
 * round a steel sink basin between the two clear ends (the curio tops), a
 * swan-neck tap over the sink, a fume-extraction arm from the wall reaching
 * over the bench with its hood, and a glass-fronted wall cabinet where
 * variant 0 has its open shelf. The kick strip carries the tag's second
 * accent.
 */
function wetBench({ k, kitAt, f, s, ctx, half, out, accent2 }: Recipe) {
  const L = LAB_BENCH;
  const W = WET_BENCH;
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const top = L.top;
  labCabinet(k, s, a0, a1, out, accent2);
  // The resin top in four pieces round the sink's opening.
  const resin = s.tinted(shade(ctx.look.palette.metal, 0.4));
  const [sa0, sa1] = W.sink.a;
  const [sd0, sd1] = W.sink.d;
  const d1 = out - 0.12;
  const t0 = top - L.resin;
  k.bevelBox(a0, sa0, 0, d1, t0, top, 0.01, resin);
  k.bevelBox(sa1, a1, 0, d1, t0, top, 0.01, resin);
  k.bevelBox(sa0, sa1, 0, sd0, t0, top, 0.01, resin);
  k.bevelBox(sa0, sa1, sd1, d1, t0, top, 0.01, resin);
  // The basin's steel floor and its drain.
  k.box(sa0, sa1, sd0, sd1, t0, t0 + 0.01, s.metal);
  k.cylinder(
    (sa0 + sa1) / 2,
    (sd0 + sd1) / 2,
    t0 + 0.01,
    t0 + 0.013,
    0.03,
    8,
    s.dark,
  );
  // The swan-neck tap: a riser and an arched neck over the basin.
  const T = W.tap;
  k.cylinder(T.a, T.d, top, T.h, 0.018, 8, s.metal);
  const neck: [number, number][] = [];
  const cd = T.d + T.reach;
  for (let i = 0; i <= 8; i++) {
    const t = (Math.PI * i) / 8;
    neck.push([
      cd - Math.cos(t) * (T.reach + 0.015),
      T.h + Math.sin(t) * (T.reach + 0.015),
    ]);
  }
  for (let i = 8; i >= 0; i--) {
    const t = (Math.PI * i) / 8;
    neck.push([
      cd - Math.cos(t) * (T.reach - 0.015),
      T.h + Math.sin(t) * (T.reach - 0.015),
    ]);
  }
  profileAlong(kitAt, f, neck, T.a - 0.015, T.a + 0.015, s.metal);
  // The fume arm: a wall plate, a duct out from the wall, an elbow, a drop
  // and the extraction hood.
  const A = W.arm;
  const [hd0, hd1] = A.hood;
  k.box(A.a - 0.08, A.a + 0.08, 0, 0.03, A.h - 0.1, A.h + 0.1, s.dark);
  kitAt(sideways(f)).cylinderAlong(-A.reach, 0, A.a, A.h, 0.04, 8, s.metal);
  k.cylinder(A.a, A.reach, A.h - 0.07, A.h + 0.06, 0.055, 10, s.dark);
  k.cylinder(A.a, A.reach, hd1, A.h - 0.07, 0.035, 8, s.metal);
  k.lathe(
    A.a,
    A.reach,
    [
      [0, hd0],
      [0.13, hd0],
      [0.04, hd1],
      [0, hd1],
    ],
    12,
    s.body,
  );
  // The glass-fronted wall cabinet: its box, two panes and the mullion.
  const C = W.cabinet;
  const [ca0, ca1] = C.a;
  const [ch0, ch1] = C.h;
  k.bevelBox(ca0, ca1, 0, C.d, ch0, ch1, 0.012, s.body);
  const glass = s.tinted(shade(ctx.look.palette.portalAlt, 0.45));
  const mid = (ca0 + ca1) / 2;
  k.panel(
    ca0 + 0.03,
    mid - 0.015,
    C.d + DECAL_LIFT,
    ch0 + 0.03,
    ch1 - 0.03,
    glass,
  );
  k.panel(
    mid + 0.015,
    ca1 - 0.03,
    C.d + DECAL_LIFT,
    ch0 + 0.03,
    ch1 - 0.03,
    glass,
  );
  k.box(mid - 0.015, mid + 0.015, C.d, C.d + 0.015, ch0, ch1, s.dark);
}

/** The analysis bench: the analyser at the far end, the reagent tower. */
const ANALYSIS_BENCH = {
  analyser: { a: [0.4, 0.86], d: [0.02, 0.2], h: 0.46 },
  tower: { a: [-0.86, -0.6], d: [0.02, 0.23], h: 1.0, shelves: 4 },
  microscope: { a: 0.1 },
} as const;

/**
 * Lab bench variant 2, an analysis bench: the same cabinet and resin top,
 * no flasks, a boxy analyser with a small glowing screen and a sample slot
 * standing on the far end's back strip (behind the curio top), a tall
 * reagent tower of shelves and bottles on the other end's back strip, and
 * a microscope between them. The kick strip carries the
 * tag's second accent.
 */
function analysisBench({ k, s, ctx, rng, half, out, accent2 }: Recipe) {
  const L = LAB_BENCH;
  const B = ANALYSIS_BENCH;
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const top = L.top;
  labCabinet(k, s, a0, a1, out, accent2);
  k.bevelBox(
    a0,
    a1,
    0,
    out - 0.12,
    top - L.resin,
    top,
    0.01,
    s.tinted(shade(ctx.look.palette.metal, 0.4)),
  );
  // The analyser: a white case, its screen and a dark sample slot.
  const A = B.analyser;
  const [aa0, aa1] = A.a;
  const [ad0, ad1] = A.d;
  k.bevelBox(aa0, aa1, ad0, ad1, top, top + A.h, 0.02, s.panel);
  k.box(
    aa0 + 0.22,
    aa1 - 0.04,
    ad1,
    ad1 + 0.01,
    top + A.h - 0.22,
    top + A.h - 0.06,
    s.dark,
  );
  k.panel(
    aa0 + 0.24,
    aa1 - 0.06,
    ad1 + 0.01 + DECAL_LIFT,
    top + A.h - 0.2,
    top + A.h - 0.08,
    s.glow(ctx.look.palette.screenText),
  );
  k.box(
    aa0 + 0.04,
    aa0 + 0.18,
    ad1,
    ad1 + 0.015,
    top + 0.08,
    top + 0.14,
    s.dark,
  );
  // The reagent tower: two side posts, its shelves and a bottle or two on each.
  const T = B.tower;
  const [ta0, ta1] = T.a;
  const [td0, td1] = T.d;
  k.box(ta0, ta0 + 0.03, td0, td1, top, top + T.h, s.metal);
  k.box(ta1 - 0.03, ta1, td0, td1, top, top + T.h, s.metal);
  k.box(ta0, ta1, td0, td0 + 0.02, top, top + T.h, s.dark);
  for (let i = 0; i < T.shelves; i++) {
    const h = top + 0.02 + i * (T.h / T.shelves);
    k.box(ta0 + 0.03, ta1 - 0.03, td0, td1, h, h + 0.02, s.metal);
    for (const a of [ta0 + 0.08, ta1 - 0.08]) {
      k.cylinder(
        a,
        (td0 + td1) / 2 + 0.02,
        h + 0.02,
        h + 0.02 + rng.range(0.1, 0.17),
        0.035,
        8,
        s.tinted(hueToRgb(rng.range(0, 360), 0.5, 0.4)),
      );
    }
  }
  k.box(ta0, ta1, td0, td1, top + T.h, top + T.h + 0.03, s.metal);
  // A microscope in the middle: foot, pillar, stage, arm and body tube.
  const M = B.microscope;
  k.bevelBox(M.a - 0.1, M.a + 0.1, 0.3, 0.52, top, top + 0.03, 0.01, s.dark);
  k.box(M.a - 0.025, M.a + 0.025, 0.3, 0.35, top + 0.03, top + 0.38, s.body);
  k.box(M.a - 0.07, M.a + 0.07, 0.35, 0.5, top + 0.12, top + 0.14, s.dark);
  k.box(M.a - 0.02, M.a + 0.02, 0.35, 0.44, top + 0.3, top + 0.36, s.body);
  k.cylinder(M.a, 0.44, top + 0.17, top + 0.42, 0.03, 8, s.metal);
}

/** The twin half racks: each cabinet's width, the gap, the unit count. */
const TWIN_RACK = {
  width: 0.35,
  gap: 0.1,
  top: 1.95,
  units: 9,
  patch: [0.9, 1.7],
  cables: 4,
} as const;

/**
 * Server rack variant 1, twin half racks: two cabinets 0.35 m wide with a
 * 0.1 m gap, each with nine unit slabs and their LEDs, and between them a
 * patch panel with a glowing row of ports and cables hanging from it to the
 * floor. Each cabinet's top trim carries the tag's second accent.
 */
function twinRacks({ k, s, ctx, hue, rng, out, accent2 }: Recipe) {
  const R = SERVER_RACK;
  const T = TWIN_RACK;
  const d1 = out - 0.1;
  const leds: Rgb[] = [
    hue,
    ctx.look.palette.screenText,
    hueToRgb(35, 0.9, 0.55),
  ];
  const g = T.gap / 2;
  for (const [c0, c1] of [
    [-g - T.width, -g],
    [g, g + T.width],
  ] as const) {
    k.bevelBox(c0, c1, 0.02, d1, 0, T.top, 0.02, s.body);
    k.box(
      c0 + 0.02,
      c1 - 0.02,
      0.1,
      d1 - 0.1,
      T.top,
      T.top + 0.03,
      trim(s.dark, accent2),
    );
    for (let u = 0; u < T.units; u++) {
      const h0 = R.firstUnit + u * R.pitch;
      k.bevelBox(
        c0 + 0.03,
        c1 - 0.03,
        d1,
        d1 + 0.02,
        h0,
        h0 + R.unit,
        0.008,
        s.dark,
      );
      for (let i = 0; i < 2; i++) {
        const a = c1 - 0.12 + i * 0.05;
        const c = leds[rng.int(0, leds.length - 1)] ?? hue;
        k.box(
          a,
          a + 0.02,
          d1 + 0.02,
          d1 + 0.026,
          h0 + 0.07,
          h0 + 0.09,
          s.glow(c),
        );
      }
    }
  }
  // The patch panel in the gap, its ports and the cables under it.
  const [p0, p1] = T.patch;
  k.box(-g, g, d1 - 0.1, d1, p0, p1, s.dark);
  for (let i = 0; i < 6; i++) {
    const h = p0 + 0.08 + i * 0.12;
    k.box(
      -0.02,
      0.02,
      d1,
      d1 + 0.006,
      h,
      h + 0.03,
      s.glow(i % 2 ? hue : ctx.look.palette.screenText),
    );
  }
  for (let i = 0; i < T.cables; i++) {
    const d = d1 - 0.085 + i * 0.022;
    const a = -0.025 + (i % 2) * 0.05;
    k.cylinder(
      a,
      d,
      0,
      p0,
      0.01,
      5,
      s.tinted(hueToRgb(200 + i * 40, 0.5, 0.35)),
    );
  }
}

/** The open frame: post size, the unit slabs, the top plate. */
const OPEN_RACK = {
  post: 0.05,
  top: 2.08,
  plate: 0.02,
  units: 10,
  cables: 3,
} as const;

/**
 * Server rack variant 2, an open frame: four thin corner posts, a top
 * plate, ten unit slabs standing open between the posts with their LEDs,
 * a bundle of cables down the left side, a status strip glowing on the top
 * plate, and the top plate's front trim in the tag's second accent.
 */
function openRack({ k, s, ctx, hue, rng, half, out, accent2 }: Recipe) {
  const R = SERVER_RACK;
  const O = OPEN_RACK;
  const [a0, a1] = [-half + 0.01, half - 0.01];
  const [d0, d1] = [0.05, out - 0.1];
  const P = O.post;
  for (const a of [a0, a1 - P])
    for (const d of [d0, d1 - P]) k.box(a, a + P, d, d + P, 0, O.top, s.metal);
  k.box(a0, a1, d0, d1, O.top, O.top + O.plate, s.metal);
  k.box(
    a0,
    a1,
    d1,
    d1 + 0.02,
    O.top - 0.03,
    O.top + O.plate,
    trim(s.dark, accent2),
  );
  k.box(
    -0.25,
    0.25,
    0.4,
    0.55,
    O.top + O.plate,
    O.top + O.plate + 0.012,
    s.glow(hue),
  );
  const leds: Rgb[] = [
    hue,
    ctx.look.palette.screenText,
    hueToRgb(35, 0.9, 0.55),
  ];
  for (let u = 0; u < O.units; u++) {
    const h0 = R.firstUnit + u * R.pitch;
    k.box(a0 + P, a1 - P, d0 + 0.03, d1 - 0.01, h0, h0 + R.unit, s.dark);
    k.box(
      a0 + P + 0.02,
      a1 - 0.25,
      d1 - 0.01,
      d1 - 0.006,
      h0 + 0.04,
      h0 + 0.12,
      s.metal,
    );
    for (let i = 0; i < R.leds; i++) {
      const a = a1 - 0.2 + i * 0.05;
      const c = leds[rng.int(0, leds.length - 1)] ?? hue;
      k.box(
        a,
        a + 0.02,
        d1 - 0.01,
        d1 - 0.004,
        h0 + 0.07,
        h0 + 0.09,
        s.glow(c),
      );
    }
  }
  // The cable bundle down the left side, clear of the posts.
  for (let i = 0; i < O.cables; i++) {
    const d = 0.3 + i * 0.1;
    k.cylinder(
      a0 + 0.015,
      d,
      0.05,
      O.top,
      0.013,
      6,
      s.tinted(hueToRgb(200 + i * 50, 0.5, 0.35)),
    );
  }
}

/** The leaning pod: the lean, the bands and the wedge plinth. */
const LEANING_POD = {
  lean: (15 * Math.PI) / 180,
  radius: 0.28,
  base: 0.3,
  bands: 5,
  band: 0.29,
  foot: 0.36,
  sides: 14,
  windows: [1, 2, 3],
} as const;

/**
 * Cryo pod variant 1, a leaning pod: the capsule tilted 15 degrees out of
 * upright on a wedge plinth, its top further from the wall than its foot.
 * The kit revolves only about upright axes, so the capsule is five short
 * bands, each set further out than the one below by the lean, joined by
 * metal rings over the steps, under a domed cap; a lit window on three of
 * the bands, feed lines and the control box as variant 0 has them. The
 * collar ring at the foot carries the tag's second accent.
 */
function leaningPod({ k, kitAt, f, s, ctx, accent2 }: Recipe) {
  const P = LEANING_POD;
  const r = P.radius;
  const step = P.band * Math.tan(P.lean);
  const centre = (i: number) => P.foot + i * step;
  const h = (i: number) => P.base + i * P.band;
  // The wedge plinth: flat under the capsule, sloping down at the front.
  profileAlong(
    kitAt,
    f,
    [
      [0.02, 0],
      [0.95, 0],
      [0.95, 0.08],
      [0.7, P.base],
      [0.02, P.base],
    ],
    -r - 0.08,
    r + 0.08,
    s.dark,
  );
  k.ring(
    0,
    centre(0),
    P.base + 0.01,
    r,
    0.035,
    6,
    P.sides,
    trim(s.metal, accent2),
    "up",
  );
  for (let i = 0; i < P.bands; i++)
    k.cylinder(0, centre(i), h(i), h(i + 1), r, P.sides, s.body);
  for (let i = 1; i < P.bands; i++)
    k.ring(
      0,
      (centre(i - 1) + centre(i)) / 2,
      h(i),
      r,
      0.045,
      6,
      P.sides,
      s.metal,
      "up",
    );
  const top = h(P.bands);
  k.lathe(
    0,
    centre(P.bands - 1),
    [
      [r, top],
      [r - 0.08, top + 0.14],
      [0.1, top + 0.2],
      [0, top + 0.21],
    ],
    P.sides,
    s.body,
  );
  // A framed window on each middle band, glowing with the cold inside.
  const front = r * Math.cos(Math.PI / P.sides);
  for (const i of P.windows) {
    const d = centre(i) + front;
    k.bevelBox(
      -0.14,
      0.14,
      d - 0.08,
      d + 0.02,
      h(i) + 0.05,
      h(i + 1) - 0.05,
      0.015,
      s.metal,
    );
    k.panel(
      -0.1,
      0.1,
      d + 0.02 + DECAL_LIFT,
      h(i) + 0.08,
      h(i + 1) - 0.08,
      s.glow(shade(ctx.look.palette.portalAlt, 0.55)),
    );
  }
  // Feed lines and a control box, as variant 0 has them.
  for (const a of [-0.2, 0.2])
    k.box(a - 0.03, a + 0.03, 0, centre(0) - r + 0.1, 0.24, 0.3, s.dark);
  k.bevelBox(-0.12, 0.12, 0, 0.06, 1.2, 1.5, 0.01, s.dark);
  k.box(
    -0.06,
    0.06,
    0.06,
    0.065,
    1.35,
    1.42,
    s.glow(ctx.look.palette.screenText),
  );
}

/** The drum pod: its radius, height and the porthole. */
const DRUM_POD = {
  radius: 0.44,
  plinth: 0.12,
  shoulder: 0.22,
  crown: 1.2,
  top: 1.42,
  porthole: 0.19,
  portHeight: 0.8,
  sides: 16,
} as const;

/**
 * Cryo pod variant 2, a drum pod: a shorter, wider capsule on a low plinth
 * with its collar ring, one round porthole glowing with the cold inside
 * behind a ring rim, a valve on the crown, and feed lines and the control
 * box as variant 0 has them. The collar ring carries the tag's second
 * accent.
 */
function drumPod({ k, s, ctx, out, accent2 }: Recipe) {
  const D = DRUM_POD;
  const d = out / 2;
  const r = Math.min(D.radius, d - 0.06);
  k.cylinder(0, d, 0, D.plinth, r + 0.03, D.sides, s.dark);
  k.ring(
    0,
    d,
    D.plinth + 0.01,
    r,
    0.04,
    6,
    D.sides,
    trim(s.metal, accent2),
    "up",
  );
  k.lathe(
    0,
    d,
    [
      [0, D.plinth],
      [r - 0.05, D.plinth],
      [r, D.shoulder],
      [r, D.crown],
      [r - 0.1, D.top - 0.08],
      [0.2, D.top - 0.01],
      [0, D.top],
    ],
    D.sides,
    s.body,
  );
  k.cylinder(0, d, D.top - 0.01, D.top + 0.08, 0.08, 10, s.metal);
  // The porthole: the glass, then its rim.
  const front = d + r * Math.cos(Math.PI / D.sides);
  const ph = D.portHeight;
  k.extrude(
    discOutline(0, ph, D.porthole, 16),
    front - 0.04,
    front + 0.015,
    s.glow(shade(ctx.look.palette.portalAlt, 0.55)),
  );
  k.ring(
    0,
    front + 0.01,
    ph,
    D.porthole + 0.02,
    0.03,
    6,
    16,
    s.metal,
    "inward",
  );
  // Feed lines and a control box, as variant 0 has them.
  for (const a of [-0.3, 0.3])
    k.box(a - 0.03, a + 0.03, 0, d - r + 0.1, 0.3, 0.36, s.dark);
  k.bevelBox(-0.12, 0.12, 0, 0.06, 1.2, 1.5, 0.01, s.dark);
  k.box(
    -0.06,
    0.06,
    0.06,
    0.065,
    1.35,
    1.42,
    s.glow(ctx.look.palette.screenText),
  );
}

/** The resin printer: the tall body, the amber lid, the build plate. */
const RESIN_PRINTER = {
  half: 0.36,
  d: [0.15, 0.75],
  body: 1.45,
  lid: 1.92,
  pane: 0.012,
  plate: 1.62,
} as const;

/**
 * Fabricator variant 1, a resin printer: a tall narrow body with a control
 * screen in a frame on its front, and on top an amber window lid (its
 * sides, back and roof, open at the front so what stands in it shows)
 * over a glowing resin vat, a build plate on its lift column and the part
 * hanging from the plate. The screen's frame carries the tag's second
 * accent.
 */
function resinPrinter({ k, s, hue, accent2 }: Recipe) {
  const P = RESIN_PRINTER;
  const x = P.half;
  const [d0, d1] = P.d;
  k.bevelBox(-x, x, d0, d1, 0, P.body, 0.03, s.body);
  // The control screen and its frame.
  k.box(-0.2, 0.2, d1, d1 + 0.02, 1.02, 1.3, trim(s.dark, accent2));
  k.panel(
    -0.17,
    0.17,
    d1 + 0.02 + DECAL_LIFT,
    1.05,
    1.27,
    s.glow(shade(hue, 0.8)),
  );
  // The amber lid: two sides, the back, the roof and a sill at the front.
  const amber = s.tinted(hueToRgb(35, 0.9, 0.5));
  const [l0, l1] = [d0 + 0.02, d1 - 0.02];
  const w = P.pane;
  k.box(-x + 0.02, -x + 0.02 + w, l0, l1, P.body, P.lid, amber);
  k.box(x - 0.02 - w, x - 0.02, l0, l1, P.body, P.lid, amber);
  k.box(-x + 0.02, x - 0.02, l0, l0 + w, P.body, P.lid, amber);
  k.box(-x + 0.02, x - 0.02, l0, l1, P.lid - w, P.lid, amber);
  k.box(-x + 0.02, x - 0.02, l1 - w, l1, P.body, P.body + 0.03, amber);
  // The resin vat and its glowing resin.
  k.box(-0.22, 0.22, 0.25, 0.62, P.body, P.body + 0.05, s.dark);
  k.box(
    -0.2,
    0.2,
    0.27,
    0.6,
    P.body + 0.05,
    P.body + 0.055,
    s.glow(shade(hue, 0.6)),
  );
  // The lift column, the build plate and the part hanging from it.
  k.box(-0.04, 0.04, l0 + w, l0 + w + 0.06, P.body, P.lid - w, s.metal);
  k.box(-0.03, 0.03, l0 + w + 0.06, 0.3, P.plate, P.plate + 0.03, s.metal);
  k.box(-0.18, 0.18, 0.3, 0.6, P.plate, P.plate + 0.02, s.metal);
  k.bevelBox(
    -0.08,
    0.06,
    0.38,
    0.52,
    P.plate - 0.08,
    P.plate,
    0.01,
    s.tinted(shade(hue, 0.7)),
  );
}

/** The arm cell: the table, the arm's joints, the part and the screen. */
const ARM_CELL = {
  table: 0.7,
  slab: 0.06,
  base: -0.35,
  depth: 0.45,
  shoulder: 1.0,
  elbow: 1.45,
  wrist: 0.1,
} as const;

/**
 * Fabricator variant 2, an arm cell: a low steel table with a controller
 * cabinet under one end, a static jointed arm standing on it (base,
 * turret, shoulder joint, upper arm, elbow joint, forearm, wrist and
 * gripper) held over a half made part, and a control screen on a post at
 * the table's right end. The screen's frame carries the tag's second
 * accent.
 */
function armCell({ k, s, hue, half, out, accent2 }: Recipe) {
  const A = ARM_CELL;
  const [a0, a1] = [-half + 0.05, half - 0.05];
  const [d0, d1] = [0.05, out - 0.05];
  const top = A.table;
  k.bevelBox(a0, a1, d0, d1, top - A.slab, top, 0.015, s.body);
  legs(k, a0 + 0.03, a1 - 0.03, d0 + 0.03, d1 - 0.03, top - A.slab, s.metal);
  k.bevelBox(
    0.3,
    a1 - 0.1,
    d0 + 0.05,
    d1 - 0.2,
    0.05,
    top - A.slab,
    0.02,
    s.dark,
  );
  // The arm, joint by joint from the table up and over the part.
  const [a, d] = [A.base, A.depth];
  k.cylinder(a, d, top, top + 0.08, 0.13, 12, s.dark);
  k.cylinder(a, d, top + 0.08, A.shoulder - 0.05, 0.09, 12, s.body);
  k.cylinderAlong(a - 0.1, a + 0.1, d, A.shoulder, 0.07, 10, s.metal);
  k.box(
    a - 0.045,
    a + 0.045,
    d - 0.045,
    d + 0.045,
    A.shoulder,
    A.elbow,
    s.body,
  );
  k.cylinderAlong(a - 0.08, a + 0.08, d, A.elbow, 0.06, 10, s.metal);
  k.box(a, A.wrist, d - 0.04, d + 0.04, A.elbow - 0.04, A.elbow + 0.04, s.body);
  k.cylinder(A.wrist, d, A.elbow - 0.2, A.elbow - 0.04, 0.035, 8, s.metal);
  k.box(
    A.wrist - 0.06,
    A.wrist + 0.06,
    d - 0.03,
    d + 0.03,
    A.elbow - 0.24,
    A.elbow - 0.2,
    s.dark,
  );
  for (const x of [A.wrist - 0.06, A.wrist + 0.045])
    k.box(
      x,
      x + 0.015,
      d - 0.02,
      d + 0.02,
      A.elbow - 0.34,
      A.elbow - 0.24,
      s.dark,
    );
  // The half made part under the gripper.
  k.bevelBox(
    A.wrist - 0.1,
    A.wrist + 0.1,
    d - 0.1,
    d + 0.1,
    top,
    top + 0.12,
    0.02,
    s.tinted(shade(hue, 0.7)),
  );
  // The control screen on its post, the frame in the tag's second accent.
  k.box(0.68, 0.72, 0.1, 0.14, top, 1.05, s.metal);
  k.box(0.58, 0.82, 0.1, 0.16, 1.05, 1.27, trim(s.dark, accent2));
  k.panel(0.6, 0.8, 0.16 + DECAL_LIFT, 1.07, 1.25, s.glow(shade(hue, 0.8)));
}

/** The tray rack: the posts, the three trays, their grow lights. */
const TRAY_RACK = {
  trays: [0.56, 1.06, 1.56],
  tray: 0.1,
  post: 1.75,
  plants: [-0.55, 0, 0.55],
} as const;

/**
 * Hydroponics variant 1, a tray rack: four corner posts holding three
 * shallow trays stacked one over another, each with its soil and a row of
 * plants, and a grow light glowing magenta under each of the upper two so
 * it lights the tray below. The lowest tray stays over the under spot's
 * clear height, which stays open. Each tray's front lip carries the tag's
 * second accent.
 */
function trayRack({ k, s, half, out, accent2 }: Recipe) {
  const T = TRAY_RACK;
  const [a0, a1] = [-half + 0.05, half - 0.05];
  const [d0, d1] = [0.08, out - 0.18];
  for (const a of [a0, a1 - 0.05])
    for (const d of [d0, d1 - 0.05])
      k.box(a, a + 0.05, d, d + 0.05, 0, T.post, s.metal);
  const leaf = s.tinted(hueToRgb(115, 0.5, 0.35));
  const dc = (d0 + d1) / 2;
  T.trays.forEach((h0, i) => {
    const h1 = h0 + T.tray;
    k.bevelBox(a0, a1, d0, d1, h0, h1, 0.015, s.body);
    k.box(
      a0 + 0.03,
      a1 - 0.03,
      d0 + 0.03,
      d1 - 0.03,
      h1,
      h1 + 0.01,
      s.tinted(hueToRgb(30, 0.4, 0.2)),
    );
    k.box(a0, a1, d1, d1 + 0.02, h0 + 0.05, h1 + 0.03, trim(s.dark, accent2));
    for (const a of T.plants)
      k.lathe(
        a,
        dc,
        [
          [0, h1],
          [0.2, h1],
          [0.16, h1 + 0.09],
          [0.08, h1 + 0.14],
          [0, h1 + 0.15],
        ],
        10,
        leaf,
      );
    if (i > 0)
      k.box(
        -0.7,
        0.7,
        dc - 0.05,
        dc + 0.05,
        h0 - 0.03,
        h0,
        s.glow(hueToRgb(300, 0.8, 0.6)),
      );
  });
}

/** The tube garden: the tank, the tubes and the grow light. */
const TUBE_GARDEN = {
  tank: 0.7,
  tankRadius: 0.15,
  depth: 0.4,
  tubes: [-0.625, -0.375, -0.125, 0.125, 0.375, 0.625],
  tubeRadius: 0.06,
  tube: [0.87, 1.95],
  pods: [1.15, 1.45, 1.75],
  light: 2.18,
} as const;

/**
 * Hydroponics variant 2, a tube garden: a nutrient tank lying along the
 * wall on two saddles (clear of the under spot), a rail along its top,
 * six upright tinted tubes standing on the rail with plants growing out of
 * their tops and out of pockets down their fronts, and a grow light
 * glowing magenta over them on arms from the wall. The tank's top rail
 * carries the tag's second accent.
 */
function tubeGarden({ k, s, ctx, half, accent2 }: Recipe) {
  const G = TUBE_GARDEN;
  const [a0, a1] = [-half + 0.06, half - 0.06];
  const d = G.depth;
  const r = G.tankRadius;
  for (const a of [a0, a1 - 0.08])
    k.box(a, a + 0.08, d - 0.18, d + 0.18, 0, G.tank, s.metal);
  k.cylinderAlong(a0, a1, d, G.tank, r, 12, s.body);
  k.box(
    a0 + 0.1,
    a1 - 0.1,
    d - 0.07,
    d + 0.07,
    G.tank + r - 0.02,
    G.tube[0],
    trim(s.dark, accent2),
  );
  const glass = s.tinted(shade(ctx.look.palette.portalAlt, 0.6));
  const leaf = s.tinted(hueToRgb(115, 0.5, 0.35));
  const [t0, t1] = G.tube;
  for (const [i, a] of G.tubes.entries()) {
    k.cylinder(a, d, t0, t1, G.tubeRadius, 10, glass);
    k.lathe(
      a,
      d,
      [
        [0, t1],
        [0.09, t1],
        [0.06, t1 + 0.08],
        [0, t1 + 0.1],
      ],
      8,
      leaf,
    );
    for (const [j, h] of G.pods.entries()) {
      if ((i + j) % 2) continue;
      k.lathe(
        a,
        d + G.tubeRadius,
        [
          [0, h - 0.03],
          [0.07, h],
          [0.05, h + 0.06],
          [0, h + 0.08],
        ],
        8,
        leaf,
      );
    }
  }
  // The grow light on two arms from the wall.
  for (const a of [a0 + 0.1, a1 - 0.14])
    k.box(a, a + 0.04, 0, d + 0.05, G.light, G.light + 0.04, s.metal);
  k.box(
    a0 + 0.05,
    a1 - 0.05,
    d - 0.05,
    d + 0.05,
    G.light - 0.04,
    G.light,
    s.glow(hueToRgb(300, 0.8, 0.6)),
  );
}

/** The nav table: pedestal, the slant's back and front heights, controls. */
const NAV_TABLE = {
  pedestal: 0.78,
  back: 1.05,
  front: 0.86,
  controls: 8,
} as const;

/**
 * Nav table: a pedestal with a slanted top tilted towards the navigator,
 * a glowing star map inset into the slope and a rim of controls.
 */
function navTable({ k, kitAt, f, s, ctx, half, out, accent2 }: Recipe) {
  const N = NAV_TABLE;
  const [a0, a1] = [-half + 0.05, half - 0.05];
  k.bevelBox(-0.55, 0.55, 0.2, 0.7, 0, N.pedestal, 0.03, s.body);
  k.bevelBox(-0.7, 0.7, 0.15, 0.75, 0, 0.08, 0.02, s.dark);
  // The slanted top, high at the back, low at the front.
  const [back, front] = [0.05, out - 0.05];
  const [hb, hf] = [N.back, N.front];
  const at = (d: number) => hb + ((d - back) / (front - back)) * (hf - hb);
  profileAlong(
    kitAt,
    f,
    [
      [back, N.pedestal],
      [front, N.pedestal],
      [front, hf],
      [back, hb],
    ],
    a0,
    a1,
    s.body,
  );
  // The map, a thin slab on the slope, and the control rim at its front.
  const [m0, m1] = [back + 0.08, front - 0.12];
  profileAlong(
    kitAt,
    f,
    [
      [m0, at(m0) - 0.01],
      [m1, at(m1) - 0.01],
      [m1, at(m1) + 0.008],
      [m0, at(m0) + 0.008],
    ],
    a0 + 0.1,
    a1 - 0.1,
    s.glow(ctx.look.palette.door),
  );
  for (let i = 0; i < N.controls; i++) {
    const a = a0 + 0.15 + i * 0.19;
    const d = front - 0.06;
    k.box(
      a,
      a + 0.08,
      d - 0.03,
      d + 0.03,
      at(d) - 0.01,
      at(d) + 0.02,
      i % 3 === 0 ? trim(s.metal, accent2) : s.dark,
    );
  }
}

/** The comms array: base, mast, dish and the blinking tip. */
const COMMS_ARRAY = {
  base: 0.4,
  mast: 2.0,
  mastRadius: 0.045,
  dish: [1.98, 2.13],
  dishRadius: 0.28,
  horn: 2.3,
  tip: 2.36,
  whips: 0.45,
  whipTop: 1.6,
} as const;

/**
 * Comms array: a narrow mast on an equipment base, a dish turned to the
 * sky with its feed horn and a blinking tip, and a pair of whip antennas.
 */
function commsArray({ k, s, hue, out, accent2 }: Recipe) {
  const C = COMMS_ARRAY;
  const d = out / 2;
  k.bevelBox(-0.3, 0.3, 0.03, out - 0.03, 0, C.base, 0.03, s.body);
  k.box(-0.2, 0.0, out - 0.03, out - 0.025, 0.2, 0.3, s.glow(shade(hue, 0.9)));
  k.cylinder(0, d, C.base, C.mast, C.mastRadius, 8, s.metal);
  k.ring(0, d, 1.2, 0.08, 0.02, 6, 10, s.dark, "up");
  const r = Math.min(C.dishRadius, d - 0.02);
  const [d0, d1] = C.dish;
  k.lathe(
    0,
    d,
    [
      [0, d0],
      [r, d1 - 0.03],
      [r, d1],
      [0, d0 + 0.04],
    ],
    16,
    s.panel,
  );
  k.cylinder(0, d, d0 + 0.04, C.horn, 0.012, 6, s.metal);
  k.cylinder(0, d, C.horn, C.tip, 0.03, 8, s.glow(hueToRgb(0, 0.9, 0.55)));
  for (const a of [-C.whips, C.whips]) {
    k.box(
      a - 0.02,
      a + 0.02,
      d - 0.02,
      d + 0.02,
      C.base,
      C.base + 0.04,
      s.dark,
    );
    k.cylinder(a, d, C.base + 0.04, C.whipTop, 0.008, 5, s.metal);
  }
  k.box(
    -C.whips,
    C.whips,
    d - 0.02,
    d + 0.02,
    C.base - 0.02,
    C.base + 0.02,
    trim(s.dark, accent2),
  );
}

/** The reactor coupling: the two drums, the core band between them. */
const REACTOR = {
  radius: 0.4,
  core: [0.8, 1.0],
  top: 1.9,
  collar: 1.6,
  sidePipe: 0.7,
  clamps: [0.4, 1.4],
} as const;

/**
 * Reactor coupling: two thick cylinders stacked and joined by collar
 * rings around a glowing core band, braced by side pipes with clamps and
 * a conduit to the wall.
 */
function reactorCoupling({ k, s, hue, half, out, accent2 }: Recipe) {
  const R = REACTOR;
  const d = out / 2;
  const r = Math.min(R.radius, d - 0.05);
  const [c0, c1] = R.core;
  k.cylinder(0, d, 0, c0, r, 16, s.body);
  k.cylinder(0, d, c0, c1, r - 0.1, 16, s.glow(hue));
  k.cylinder(0, d, c1, R.top, r, 16, s.body);
  const collar = trim(s.metal, accent2);
  k.ring(0, d, c0, r - 0.06, 0.05, 6, 16, collar, "up");
  k.ring(0, d, c1, r - 0.06, 0.05, 6, 16, collar, "up");
  k.ring(0, d, R.collar, r, 0.03, 6, 16, s.dark, "up");
  const side = Math.min(R.sidePipe, half - 0.1);
  for (const a of [-side, side]) {
    k.cylinder(a, d, 0, R.top, 0.07, 8, s.metal);
    for (const h of R.clamps) {
      k.box(
        Math.min(a, 0),
        Math.max(a, 0),
        d - 0.04,
        d + 0.04,
        h,
        h + 0.08,
        s.dark,
      );
    }
  }
  k.box(-0.1, 0.1, 0, d, R.top - 0.2, R.top - 0.1, s.dark);
}

/** The cargo loader: mast, carriage and fork heights, the crate. */
const CARGO_LOADER = {
  mast: 1.8,
  base: 0.12,
  carriage: [0.3, 0.45],
  fork: 0.05,
  forks: [-0.45, 0.3],
  forkWidth: 0.15,
  crate: [0.35, 0.85],
  crateHalf: 0.55,
  beacon: 0.1,
} as const;

/**
 * Cargo loader: a mast frame with a carriage and two forks carrying a
 * crate, hydraulic rams beside the mast and a warning beacon on top.
 */
function cargoLoader({ k, s, half, out, accent2 }: Recipe) {
  const C = CARGO_LOADER;
  const m = Math.min(0.75, half - 0.05);
  k.bevelBox(-m - 0.1, m + 0.1, 0, 0.25, 0, C.base, 0.02, s.dark);
  for (const a of [-m, m - 0.1])
    k.bevelBox(a, a + 0.1, 0.05, 0.2, C.base, C.mast, 0.015, s.body);
  k.bevelBox(
    -m,
    m,
    0.05,
    0.2,
    C.mast - 0.1,
    C.mast,
    0.015,
    trim(s.body, accent2),
  );
  // The carriage and its forks.
  const [c0, c1] = C.carriage;
  k.box(-m + 0.1, m - 0.1, 0.2, 0.26, c0, c1, s.metal);
  for (const a of C.forks) {
    k.box(a, a + C.forkWidth, 0.2, 0.26, c0, 0.8, s.metal);
    k.box(a, a + C.forkWidth, 0.26, out - 0.02, c0, c0 + C.fork, s.metal);
  }
  const [k0, k1] = C.crate;
  k.bevelBox(
    -C.crateHalf,
    C.crateHalf,
    0.3,
    out - 0.04,
    k0,
    k1,
    0.02,
    s.tinted(hueToRgb(30, 0.55, 0.45)),
  );
  k.box(
    -C.crateHalf - 0.01,
    C.crateHalf + 0.01,
    0.3,
    out - 0.04,
    0.55,
    0.6,
    s.hazard,
  );
  // Hydraulic rams and the beacon.
  for (const a of [-m + 0.2, m - 0.2])
    k.cylinder(a, 0.12, C.base, 1.2, 0.03, 8, s.metal);
  k.cylinder(
    m - 0.05,
    0.12,
    C.mast,
    C.mast + C.beacon,
    0.05,
    10,
    s.glow(hueToRgb(38, 0.95, 0.55)),
  );
}

/** The med scanner: bed height, the arch's radii and extent, the monitor. */
const MED_SCANNER = {
  bed: 0.62,
  pad: 0.07,
  archOuter: 0.02,
  archWidth: 0.1,
  arch: [0.05, 0.2],
  archFacets: 10,
  bar: [0.08, 0.17],
  monitor: [1.2, 1.5],
} as const;

/**
 * Med scanner: a padded bed on a pedestal, a half-ring arch standing over
 * it on rails with a glowing scan bar under its crown, and a vitals
 * monitor on the wall.
 */
function medScanner({ k, kitAt, f, s, ctx, hue, half, out, accent2 }: Recipe) {
  const M = MED_SCANNER;
  const [a0, a1] = [-half + 0.05, half - 0.05];
  const dc = out / 2;
  const bed = M.bed;
  k.bevelBox(-0.4, 0.4, dc - 0.2, dc + 0.2, 0, bed - 0.08, 0.03, s.dark);
  k.bevelBox(a0, a1, dc - 0.35, dc + 0.35, bed - 0.08, bed, 0.02, s.body);
  k.bevelBox(
    a0 + 0.05,
    a1 - 0.05,
    dc - 0.31,
    dc + 0.31,
    bed,
    bed + M.pad,
    0.03,
    s.panel,
  );
  k.bevelBox(
    a0 + 0.1,
    a0 + 0.4,
    dc - 0.2,
    dc + 0.2,
    bed + M.pad,
    bed + M.pad + 0.05,
    0.025,
    s.panel,
  );
  // The rails along both sides of the bed, and the arch on them.
  const ro = dc - M.archOuter;
  const ri = ro - M.archWidth;
  const base = bed - 0.04;
  for (const d of [dc - ro, dc + ro - 0.08])
    k.box(a0, a1, d, d + 0.08, bed - 0.06, bed - 0.02, trim(s.metal, accent2));
  const arch: [number, number][] = [];
  for (let i = 0; i <= M.archFacets; i++) {
    const t = (Math.PI * i) / M.archFacets;
    arch.push([dc - Math.cos(t) * ro, base + Math.sin(t) * ro]);
  }
  for (let i = M.archFacets; i >= 0; i--) {
    const t = (Math.PI * i) / M.archFacets;
    arch.push([dc - Math.cos(t) * ri, base + Math.sin(t) * ri]);
  }
  const [x0, x1] = M.arch;
  profileAlong(kitAt, f, arch, x0, x1, s.body);
  // The scan bar under the crown of the arch.
  const crown = base + ri;
  const [b0, b1] = M.bar;
  k.box(b0, b1, dc - 0.3, dc + 0.3, crown - 0.03, crown + 0.005, s.glow(hue));
  // The vitals monitor.
  const [m0, m1] = M.monitor;
  k.bevelBox(a0 + 0.05, a0 + 0.45, 0, 0.08, m0, m1, 0.015, s.dark);
  k.panel(
    a0 + 0.08,
    a0 + 0.42,
    0.08 + DECAL_LIFT,
    m0 + 0.03,
    m1 - 0.03,
    s.glow(ctx.look.palette.screenText),
  );
}

/** Containment: base, tank, ribs, cap; the glass shell and the core in it. */
const CONTAINMENT = {
  radius: 0.4,
  base: 0.3,
  tank: 1.7,
  cap: 1.95,
  glass: 0.33,
  core: 0.24,
  ribs: [0.55, 0.85, 1.15, 1.45],
  gap: 0.05,
  sides: 16,
} as const;

/**
 * Containment: a tank on a heavy base, a tinted glass shell held by ribs
 * (rings round it) and vertical struts, and a cap with a conduit to the
 * wall. The glowing core inside runs the tank's full height, and shows as
 * bright bands in the gaps where the glass stops short of the base and the
 * cap.
 */
function containment({ k, s, hue, out, accent2 }: Recipe) {
  const C = CONTAINMENT;
  const d = out / 2;
  const r = Math.min(C.radius, d - 0.03);
  k.cylinder(0, d, 0, C.base, r, C.sides, s.body);
  k.cylinder(0, d, C.base, C.tank, C.core, C.sides, s.glow(hue));
  const [g0, g1] = [C.base + C.gap, C.tank - C.gap];
  k.lathe(
    0,
    d,
    [
      [0, g0],
      [C.glass, g0],
      [C.glass, g1],
      [0, g1],
    ],
    C.sides,
    s.glow(shade(hue, 0.3)),
  );
  for (const h of C.ribs)
    k.ring(0, d, h, C.glass + 0.02, 0.025, 6, C.sides, s.metal, "up");
  for (let i = 0; i < 4; i++) {
    const t = (Math.PI / 4) * (1 + 2 * i);
    const [a, dd] = [
      Math.sin(t) * (C.glass + 0.03),
      d + Math.cos(t) * (C.glass + 0.03),
    ];
    k.box(a - 0.02, a + 0.02, dd - 0.02, dd + 0.02, C.base, C.tank, s.dark);
  }
  k.cylinder(0, d, C.tank, C.cap, r, C.sides, s.body);
  k.cylinder(0, d, C.cap, C.cap + 0.05, r - 0.12, 12, trim(s.dark, accent2));
  k.box(-0.08, 0.08, 0, d, C.cap - 0.15, C.cap - 0.05, s.dark);
}

/** The round plotting table: the top, the pedestal, the dome of stars. */
const PLOTTING_TABLE = {
  radius: 0.45,
  top: [0.84, 0.9],
  foot: 0.34,
  map: 0.38,
  dome: 0.2,
  stars: 10,
  sides: 24,
} as const;

/**
 * Nav table variant 1, a round plotting table: a round top as wide as the
 * footprint is deep on a flared round pedestal and foot, a glowing star
 * map inset in the top with stars scattered over it, and a glowing dome of
 * stars over its centre. The rim ring on the top's edge carries the tag's
 * second accent.
 */
function plottingTable({ k, s, ctx, rng, out, accent2 }: Recipe) {
  const P = PLOTTING_TABLE;
  const d = out / 2;
  const r = Math.min(P.radius, d);
  const [t0, t1] = P.top;
  k.cylinder(0, d, 0, 0.08, P.foot, P.sides, s.dark);
  k.lathe(
    0,
    d,
    [
      [0, 0.08],
      [0.22, 0.08],
      [0.14, 0.26],
      [0.13, 0.66],
      [0.26, t0],
      [0, t0],
    ],
    P.sides,
    s.body,
  );
  k.cylinder(0, d, t0, t1, r, P.sides, s.body);
  k.ring(0, d, t1, r - 0.03, 0.025, 6, P.sides, trim(s.metal, accent2), "up");
  // The star map, the stars over it and the dome over its centre.
  const map = ctx.look.palette.door;
  k.cylinder(0, d, t1, t1 + 0.005, P.map, P.sides, s.glow(shade(map, 0.55)));
  for (let i = 0; i < P.stars; i++) {
    const angle = rng.range(0, 2 * Math.PI);
    const at = rng.range(P.dome + 0.04, P.map - 0.04);
    const [a, dd] = [Math.sin(angle) * at, d + Math.cos(angle) * at];
    k.box(
      a - 0.01,
      a + 0.01,
      dd - 0.01,
      dd + 0.01,
      t1,
      t1 + 0.012,
      s.glow(ctx.look.palette.screenText),
    );
  }
  const dome: [number, number][] = [];
  for (let i = 0; i <= 6; i++) {
    const t = (Math.PI / 2) * (i / 6);
    dome.push([P.dome * Math.cos(t), t1 + P.dome * Math.sin(t)]);
  }
  k.lathe(0, d, [[0, t1], ...dome], 16, s.glow(map));
}

/** The chart lectern: its side profile, the chart's reach, the lean rail. */
const CHART_LECTERN = {
  half: 0.6,
  base: [0.1, 0.5],
  lip: [0.62, 1.15],
  crown: [0.14, 1.62],
  chart: [0.2, 0.56],
  chartHalf: 0.52,
  rail: [0.82, 0.68],
} as const;

/**
 * Nav table variant 2, a chart lectern: a tall lectern on a plinth, its top
 * sloping down from the wall towards the navigator, a wide glowing chart
 * panel on the slope, a row of keys and a status light on its front, and a
 * lean rail at stool height in front of it on two brackets. The rim along
 * the top's front lip carries the tag's second accent.
 */
function chartLectern({ k, kitAt, f, s, ctx, hue, accent2 }: Recipe) {
  const L = CHART_LECTERN;
  const x = L.half;
  const [b0, b1] = L.base;
  const [ld, lh] = L.lip;
  const [cd, ch] = L.crown;
  k.bevelBox(-x - 0.08, x + 0.08, 0.04, b1 + 0.1, 0, 0.08, 0.02, s.dark);
  profileAlong(
    kitAt,
    f,
    [
      [b0, 0.08],
      [b1, 0.08],
      [b1, 0.95],
      [ld, lh],
      [cd, ch],
      [b0, ch],
    ],
    -x,
    x,
    s.body,
  );
  // The chart on the slope and the rim along the lip.
  const at = (d: number) => lh + ((ld - d) / (ld - cd)) * (ch - lh);
  const [m0, m1] = L.chart;
  profileAlong(
    kitAt,
    f,
    [
      [m0, at(m0) - 0.01],
      [m1, at(m1) - 0.01],
      [m1, at(m1) + 0.008],
      [m0, at(m0) + 0.008],
    ],
    -L.chartHalf,
    L.chartHalf,
    s.glow(ctx.look.palette.door),
  );
  k.box(
    -x,
    x,
    ld - 0.04,
    ld + 0.02,
    lh - 0.05,
    lh + 0.01,
    trim(s.metal, accent2),
  );
  // Keys and a status light on the front face.
  for (let i = 0; i < 5; i++) {
    const a = -0.4 + i * 0.2;
    k.box(a - 0.04, a + 0.04, b1, b1 + 0.02, 0.8, 0.84, s.dark);
  }
  k.box(-0.2, 0.2, b1, b1 + 0.01, 0.62, 0.65, s.glow(shade(hue, 0.9)));
  // The lean rail on its two brackets.
  const [rd, rh] = L.rail;
  for (const a of [-x + 0.1, x - 0.14])
    k.box(a, a + 0.04, b1, rd, rh - 0.03, rh + 0.01, s.metal);
  k.cylinderAlong(-x, x, rd, rh, 0.025, 8, s.metal);
}

/** The lattice mast: its base, posts, rungs, cross antennas and tip. */
const LATTICE_MAST = {
  base: 0.25,
  half: 0.1,
  bar: 0.03,
  top: 2.1,
  rungs: [0.45, 0.85, 1.25, 1.65, 2.05],
  cross: [1.45, 1.75, 1.95],
  crossHalf: 0.42,
  tip: [2.26, 2.32],
} as const;

/**
 * Comms array variant 1, a lattice mast: a thin square mast of four corner
 * posts on an equipment base, a rung on every face at five levels and a
 * zigzag of braces up its front and back, three short cross antennas with
 * upright tips near the top, and a thin rod over a cap with the tip
 * glowing red at its end. The base's trim band carries the tag's second
 * accent.
 */
function latticeMast({ k, s, hue, out, accent2 }: Recipe) {
  const M = LATTICE_MAST;
  const d = out / 2;
  const [x, w] = [M.half, M.bar / 2];
  k.bevelBox(-0.3, 0.3, 0.05, out - 0.05, 0, M.base, 0.03, s.body);
  k.box(
    -0.31,
    0.31,
    out - 0.06,
    out - 0.04,
    M.base - 0.07,
    M.base - 0.03,
    trim(s.dark, accent2),
  );
  k.box(
    -0.2,
    0.0,
    out - 0.05,
    out - 0.045,
    0.08,
    0.14,
    s.glow(shade(hue, 0.9)),
  );
  // The four posts, the rungs and the braces.
  const corners = [-x, x];
  const depths = [d - x, d + x];
  for (const a of corners)
    for (const dd of depths)
      k.box(a - w, a + w, dd - w, dd + w, M.base, M.top, s.metal);
  for (const h of M.rungs) {
    for (const dd of depths)
      k.box(-x, x, dd - w + 0.005, dd + w - 0.005, h - 0.01, h + 0.01, s.metal);
    for (const a of corners)
      k.box(
        a - w + 0.005,
        a + w - 0.005,
        d - x,
        d + x,
        h - 0.01,
        h + 0.01,
        s.metal,
      );
  }
  for (let i = 0; i + 1 < M.rungs.length; i++) {
    const [h0, h1] = [M.rungs[i] ?? 0, M.rungs[i + 1] ?? 0];
    const angle = Math.atan2(h1 - h0, i % 2 ? -2 * x : 2 * x);
    const bar = tiltedBar(
      0,
      (h0 + h1) / 2,
      angle,
      Math.hypot(2 * x, h1 - h0),
      0.018,
    );
    for (const dd of depths) k.extrude(bar, dd - 0.008, dd + 0.008, s.dark);
  }
  // The cross antennas, each with a short upright tip at both ends.
  for (const h of M.cross) {
    k.cylinderAlong(-M.crossHalf, M.crossHalf, d, h, 0.012, 6, s.metal);
    for (const a of [-M.crossHalf + 0.02, M.crossHalf - 0.02])
      k.cylinder(a, d, h - 0.1, h + 0.1, 0.008, 5, s.metal);
  }
  // The cap, the rod and the glowing tip.
  const [g0, g1] = M.tip;
  k.box(-x - w, x + w, d - x - w, d + x + w, M.top, M.top + 0.03, s.dark);
  k.cylinder(0, d, M.top + 0.03, g0, 0.012, 6, s.metal);
  k.cylinder(0, d, g0, g1, 0.03, 8, s.glow(hueToRgb(0, 0.9, 0.55)));
}

/** The radio rack: the rack, its three sets, the dish and the microphone. */
const RADIO_RACK = {
  half: 0.5,
  d: [0.06, 0.48],
  top: 1.2,
  sets: [
    [0.14, 0.42],
    [0.48, 0.76],
    [0.82, 1.1],
  ],
  dish: { a: 0.2, d: 0.27, r: 0.22 },
  mic: { a: -0.3, d: 0.32, stem: 1.4 },
} as const;

/**
 * Comms array variant 2, a radio rack: a squat rack on a plinth holding
 * three radio sets, each with a lit dial window and two tuning knobs, a
 * dish folded flat on the rack's top on its hinge block, and a desk
 * microphone beside it. Nothing reaches above 1.6 m. The plinth's trim
 * band carries the tag's second accent.
 */
function radioRack({ k, s, ctx, hue, accent2 }: Recipe) {
  const R = RADIO_RACK;
  const x = R.half;
  const [d0, d1] = R.d;
  const top = R.top;
  k.bevelBox(-x - 0.03, x + 0.03, d0 - 0.03, d1 + 0.04, 0, 0.08, 0.02, s.dark);
  k.box(
    -x - 0.04,
    x + 0.04,
    d1 + 0.04,
    d1 + 0.06,
    0.02,
    0.06,
    trim(s.dark, accent2),
  );
  k.bevelBox(-x, x, d0, d1, 0.08, top, 0.025, s.body);
  // The radio sets: a face plate, a lit dial window and two knobs.
  for (const [h0, h1] of R.sets) {
    k.bevelBox(-x + 0.04, x - 0.04, d1, d1 + 0.03, h0, h1, 0.01, s.dark);
    const f = d1 + 0.03;
    k.panel(
      -0.36,
      0.06,
      f + DECAL_LIFT,
      h0 + 0.08,
      h1 - 0.08,
      s.glow(shade(hue, 0.8)),
    );
    for (const a of [0.2, 0.35]) {
      k.extrude(discOutline(a, (h0 + h1) / 2, 0.045, 10), f, f + 0.03, s.metal);
      k.box(
        a - 0.005,
        a + 0.005,
        f + 0.03,
        f + 0.035,
        (h0 + h1) / 2,
        (h0 + h1) / 2 + 0.04,
        s.dark,
      );
    }
  }
  // The folded dish lying on its hinge block, face up.
  const D = R.dish;
  k.box(
    D.a - 0.08,
    D.a + 0.08,
    D.d - 0.05,
    D.d + 0.05,
    top,
    top + 0.05,
    s.dark,
  );
  k.lathe(
    D.a,
    D.d,
    [
      [0, top + 0.05],
      [D.r, top + 0.1],
      [D.r, top + 0.12],
      [0, top + 0.08],
    ],
    16,
    s.panel,
  );
  k.cylinder(D.a, D.d, top + 0.08, top + 0.2, 0.01, 6, s.metal);
  // The desk microphone: foot, stem and head.
  const M = R.mic;
  k.cylinder(M.a, M.d, top, top + 0.02, 0.06, 10, s.dark);
  k.cylinder(M.a, M.d, top + 0.02, M.stem, 0.008, 6, s.metal);
  k.lathe(
    M.a,
    M.d,
    [
      [0, M.stem],
      [0.03, M.stem + 0.01],
      [0.03, M.stem + 0.07],
      [0, M.stem + 0.08],
    ],
    10,
    s.dark,
  );
  k.box(
    -x + 0.06,
    -x + 0.2,
    d1,
    d1 + 0.005,
    1.14,
    1.17,
    s.glow(ctx.look.palette.screenText),
  );
}

/** The lying coupling: the drums, the core between them, the saddles. */
const LYING_COUPLING = {
  radius: 0.4,
  axis: 0.62,
  gap: 0.14,
  end: 0.85,
  core: 0.26,
  saddles: [0.3, 0.65],
  pipes: 0.5,
  elbow: 1.3,
} as const;

/**
 * Reactor coupling variant 1, a lying coupling: two thick drums lying
 * along the wall end to end on saddles, a short glowing core between
 * them, dark end plates and bands, and a riser pipe from each drum's back
 * that bends into the wall on a flange. The collar ring at each drum's
 * inner end carries the tag's second accent.
 */
function lyingCoupling({ k, kitAt, f, s, hue, out, accent2 }: Recipe) {
  const C = LYING_COUPLING;
  const d = out / 2;
  const r = Math.min(C.radius, d - 0.05);
  const h = C.axis;
  const side = kitAt(sideways(f));
  // In the quarter-turned frame a ring facing `inward` stands across the
  // wall, its axis along it: `(a', d')` there is `(d', -a')` here.
  const across = (a: number, radius: number, tube: number, sf: Surface) =>
    side.ring(-d, a, h, radius, tube, 6, 16, sf, "inward");
  for (const sign of [-1, 1]) {
    const [i, o] = [sign * C.gap, sign * C.end];
    k.cylinderAlong(i, o, d, h, r, 16, s.body);
    k.cylinderAlong(o, o + sign * 0.03, d, h, r - 0.05, 16, s.dark);
    across(sign * C.pipes, r, 0.025, s.dark);
    across(i, C.core + 0.04, 0.05, trim(s.metal, accent2));
    for (const a of C.saddles)
      k.box(
        sign * a - 0.05,
        sign * a + 0.05,
        d - 0.3,
        d + 0.3,
        0,
        h - r + 0.08,
        s.dark,
      );
    // The riser from the drum's back, its elbow and flange on the wall.
    const p = sign * C.pipes;
    k.cylinder(p, 0.12, h, C.elbow + 0.06, 0.06, 8, s.metal);
    side.cylinderAlong(-0.12, 0, p, C.elbow, 0.06, 8, s.metal);
    k.box(p - 0.1, p + 0.1, 0, 0.03, C.elbow - 0.1, C.elbow + 0.1, s.dark);
  }
  k.cylinderAlong(-C.gap, C.gap, d, h, C.core, 16, s.glow(hue));
}

/** The sphere vessel: the sphere, its ring stand, the band and the neck. */
const SPHERE_VESSEL = {
  radius: 0.4,
  centre: 1.05,
  stand: 0.75,
  standRing: 0.28,
  legs: 0.28,
  neck: [1.43, 1.56],
  pipes: [
    [-0.18, 1.2],
    [0.18, 0.88],
  ],
} as const;

/**
 * Reactor coupling variant 2, a sphere vessel: a sphere held in a ring
 * stand on four legs over a round foot plate, a glowing band round its
 * middle, two pipes from its back into the wall on flanges, and a neck on
 * its crown with a pipe back to the wall. The stand's ring and the neck's
 * collar carry the tag's second accent.
 */
function sphereVessel({ k, kitAt, f, s, hue, out, accent2 }: Recipe) {
  const V = SPHERE_VESSEL;
  const d = out / 2;
  const r = Math.min(V.radius, d - 0.05);
  const c = V.centre;
  k.cylinder(0, d, 0, 0.05, 0.38, 16, s.dark);
  for (let i = 0; i < 4; i++) {
    const t = (Math.PI / 4) * (1 + 2 * i);
    const [a, dd] = [Math.sin(t) * V.legs, d + Math.cos(t) * V.legs];
    k.box(a - 0.025, a + 0.025, dd - 0.025, dd + 0.025, 0.05, V.stand, s.metal);
  }
  k.ring(0, d, V.stand, V.standRing, 0.04, 6, 16, trim(s.metal, accent2), "up");
  const sphere: [number, number][] = [];
  for (let i = 0; i <= 10; i++) {
    const t = Math.PI * (i / 10 - 0.5);
    sphere.push([r * Math.cos(t), c + r * Math.sin(t)]);
  }
  k.lathe(0, d, sphere, 16, s.body);
  k.ring(0, d, c, r, 0.035, 6, 16, s.glow(hue), "up");
  // The neck, its collar and the pipe from it back into the wall.
  const [n0, n1] = V.neck;
  const side = kitAt(sideways(f));
  k.cylinder(0, d, n0 - 0.05, n1, 0.08, 10, s.metal);
  k.ring(0, d, n1 - 0.04, 0.09, 0.025, 6, 10, trim(s.dark, accent2), "up");
  side.cylinderAlong(-d, 0, 0, n1 - 0.05, 0.045, 8, s.metal);
  k.box(-0.08, 0.08, 0, 0.03, n1 - 0.12, n1 + 0.02, s.dark);
  // Two pipes from the sphere's back into the wall, each on a flange.
  for (const [a, h] of V.pipes) {
    side.cylinderAlong(-0.22, 0, a, h, 0.05, 8, s.metal);
    k.box(a - 0.09, a + 0.09, 0, 0.03, h - 0.09, h + 0.09, s.dark);
  }
}

/** The scissor lift: the frame, the two scissor stages, the platform. */
const SCISSOR_LIFT = {
  half: 0.8,
  d: [0.1, 0.8],
  base: 0.1,
  deck: [0.66, 0.74],
  arm: 0.65,
  stages: [
    [0.14, 0.38],
    [0.42, 0.62],
  ],
  crate: { half: 0.42, d: [0.22, 0.72], top: 1.22 },
  rail: 1.28,
} as const;

/**
 * Cargo loader variant 1, a scissor lift: a low base frame, two stages of
 * crossed arms on each side with pivot pins through the crossings, a deck
 * on top with a guard rail along its back, and a crate strapped on the
 * deck; a warning beacon glows on the rail's right post. The beacon's
 * housing carries the tag's second accent.
 */
function scissorLift({ k, kitAt, f, s, accent2 }: Recipe) {
  const L = SCISSOR_LIFT;
  const x = L.half;
  const [d0, d1] = L.d;
  const [k0, k1] = L.deck;
  k.bevelBox(-x, x, d0, d1, 0, L.base, 0.015, s.dark);
  k.bevelBox(-x, x, d0, d1, k0, k1, 0.015, s.body);
  // The arms: each stage an X, its two arms side by side on each face.
  const side = kitAt(sideways(f));
  const faces = [d0 + 0.02, d1 - 0.08];
  for (const [h0, h1] of L.stages) {
    const mid = (h0 + h1) / 2;
    const angle = Math.atan2(h1 - h0, 2 * L.arm);
    const length = Math.hypot(2 * L.arm, h1 - h0) + 0.05;
    for (const face of faces) {
      k.extrude(
        tiltedBar(0, mid, angle, length, 0.05),
        face,
        face + 0.03,
        s.metal,
      );
      k.extrude(
        tiltedBar(0, mid, -angle, length, 0.05),
        face + 0.03,
        face + 0.06,
        s.metal,
      );
    }
    side.cylinderAlong(-d1 + 0.02, -d0 - 0.02, 0, mid, 0.02, 8, s.dark);
    for (const a of [-L.arm, L.arm])
      for (const h of [h0, h1])
        side.cylinderAlong(-d1 + 0.02, -d0 - 0.02, a, h, 0.018, 6, s.dark);
  }
  // The crate, strapped, and the rail along the deck's back.
  const C = L.crate;
  const [c0, c1] = C.d;
  k.bevelBox(
    -C.half,
    C.half,
    c0,
    c1,
    k1,
    C.top,
    0.02,
    s.tinted(hueToRgb(30, 0.55, 0.45)),
  );
  for (const a of [-0.2, 0.2])
    k.box(a - 0.03, a + 0.03, c0 - 0.01, c1 + 0.01, k1, C.top + 0.01, s.dark);
  for (const a of [-x + 0.02, x - 0.06])
    k.box(a, a + 0.04, d0 + 0.02, d0 + 0.06, k1, L.rail, s.metal);
  k.cylinderAlong(
    -x + 0.02,
    x - 0.02,
    d0 + 0.04,
    L.rail - 0.02,
    0.02,
    8,
    s.metal,
  );
  // The beacon on the right post: its housing, then the lens.
  const b = x - 0.04;
  k.cylinder(
    b,
    d0 + 0.04,
    L.rail,
    L.rail + 0.06,
    0.05,
    10,
    trim(s.dark, accent2),
  );
  k.cylinder(
    b,
    d0 + 0.04,
    L.rail + 0.06,
    L.rail + 0.14,
    0.04,
    10,
    s.glow(hueToRgb(38, 0.95, 0.55)),
  );
}

/** The gantry hoist: the frame, the beam, the chain and the hanging crate. */
const GANTRY_HOIST = {
  half: 0.85,
  post: 0.08,
  d: [0.1, 0.8],
  beam: [1.9, 2.05],
  trolley: 1.62,
  links: 8,
  link: 0.06,
  hook: 1.08,
  crate: { half: 0.35, d: [0.2, 0.7], h: [0.5, 0.95] },
} as const;

/**
 * Cargo loader variant 2, a gantry hoist: two end frames of posts on foot
 * rails joined by a beam overhead, a trolley and hoist motor under the
 * beam, a chain down from it to a hook, a spreader bar and two slings
 * holding a crate that hangs clear of the floor, and a control pendant
 * on its cable. A warning beacon glows on the beam; its housing carries
 * the tag's second accent.
 */
function gantryHoist({ k, s, accent2 }: Recipe) {
  const G = GANTRY_HOIST;
  const [x, p] = [G.half, G.post];
  const [d0, d1] = G.d;
  const [b0, b1] = G.beam;
  const dc = (d0 + d1) / 2;
  for (const a of [-x, x - p]) {
    k.box(a, a + p, d0 - 0.05, d1 + 0.05, 0, 0.08, s.dark);
    for (const dd of [d0, d1 - p])
      k.bevelBox(a, a + p, dd, dd + p, 0.08, b0, 0.01, s.body);
    k.box(a, a + p, d0, d1, b0 - 0.08, b0, s.body);
  }
  k.bevelBox(-x, x, dc - 0.06, dc + 0.06, b0, b1, 0.015, s.body);
  // The trolley under the beam, the hoist motor and the chain.
  k.box(-0.15, 0.15, dc - 0.09, dc + 0.09, b0 - 0.1, b0, s.dark);
  k.bevelBox(
    -0.12,
    0.12,
    dc - 0.07,
    dc + 0.07,
    G.trolley,
    b0 - 0.1,
    0.01,
    s.metal,
  );
  for (let i = 0; i < G.links; i++) {
    const h1 = G.trolley - i * G.link;
    const wide = i % 2 === 0;
    const [ah, dh] = wide ? [0.02, 0.006] : [0.006, 0.02];
    k.box(-ah, ah, dc - dh, dc + dh, h1 - G.link - 0.01, h1, s.dark);
  }
  // The hook, the spreader bar, the slings and the crate.
  const hook = G.trolley - G.links * G.link;
  k.box(-0.02, 0.02, dc - 0.02, dc + 0.02, G.hook, hook, s.metal);
  k.box(-0.05, 0.05, dc - 0.015, dc + 0.015, G.hook - 0.03, G.hook, s.metal);
  const C = G.crate;
  const [c0, c1] = C.h;
  k.box(
    -C.half,
    C.half,
    dc - 0.02,
    dc + 0.02,
    G.hook - 0.07,
    G.hook - 0.03,
    s.metal,
  );
  for (const a of [-C.half + 0.03, C.half - 0.05])
    k.box(a, a + 0.02, dc - 0.01, dc + 0.01, c1, G.hook - 0.07, s.dark);
  const [e0, e1] = C.d;
  k.bevelBox(
    -C.half,
    C.half,
    e0,
    e1,
    c0,
    c1,
    0.02,
    s.tinted(hueToRgb(30, 0.55, 0.45)),
  );
  k.box(
    -C.half - 0.01,
    C.half + 0.01,
    e0 - 0.01,
    e1 + 0.01,
    c0 + 0.18,
    c0 + 0.23,
    s.hazard,
  );
  // The pendant on its cable, and the beacon on the beam.
  k.box(0.14, 0.15, dc - 0.005, dc + 0.005, 1.3, b0 - 0.1, s.dark);
  k.bevelBox(0.11, 0.18, dc - 0.04, dc + 0.04, 1.12, 1.3, 0.01, s.metal);
  k.cylinder(x - 0.2, dc, b1, b1 + 0.05, 0.05, 10, trim(s.dark, accent2));
  k.cylinder(
    x - 0.2,
    dc,
    b1 + 0.05,
    b1 + 0.13,
    0.04,
    10,
    s.glow(hueToRgb(38, 0.95, 0.55)),
  );
}

/** The ring scanner: the ring, its lining, the bed and its pedestal. */
const RING_SCANNER = {
  ring: [0.3, 0.62],
  centre: 0.74,
  outer: 0.44,
  inner: 0.3,
  lining: [0.44, 0.48],
  facets: 12,
  bed: [0.6, 0.66],
  pad: 0.72,
  bedA: [-0.85, 0.78],
  pedestal: [-0.75, -0.25],
  monitor: [1.2, 1.5],
} as const;

/** Half an annulus in the `(d, h)` plane, from `from` to `from + PI`. */
function halfAnnulus(
  d: number,
  h: number,
  outer: number,
  inner: number,
  from: number,
  facets: number,
): [number, number][] {
  const at = (r: number, t: number): [number, number] => [
    d + Math.cos(t) * r,
    h + Math.sin(t) * r,
  ];
  const out: [number, number][] = [];
  for (let i = 0; i <= facets; i++)
    out.push(at(outer, from + (Math.PI * i) / facets));
  for (let i = facets; i >= 0; i--)
    out.push(at(inner, from + (Math.PI * i) / facets));
  return out;
}

/**
 * Med scanner variant 1, a ring scanner: a thick upright ring standing
 * across the bed on its feet, its inside lined with a glowing band, and a
 * padded bed on a pedestal at the far end reaching through the ring as if
 * slid into it, with a pillow, and the vitals monitor on the wall. The
 * pedestal's trim band carries the tag's second accent.
 */
function ringScanner({ k, kitAt, f, s, ctx, hue, out, accent2 }: Recipe) {
  const M = RING_SCANNER;
  const dc = out / 2;
  const c = M.centre;
  const [x0, x1] = M.ring;
  // The ring in two halves (an extruded outline cannot have a hole), its
  // feet, and the glowing lining inside it.
  for (const from of [0, Math.PI])
    profileAlong(
      kitAt,
      f,
      halfAnnulus(dc, c, M.outer, M.inner, from, M.facets),
      x0,
      x1,
      s.body,
    );
  k.bevelBox(
    x0 + 0.02,
    x1 - 0.02,
    dc - 0.3,
    dc + 0.3,
    0,
    c - M.outer + 0.06,
    0.02,
    s.dark,
  );
  const [l0, l1] = M.lining;
  for (const from of [0, Math.PI])
    profileAlong(
      kitAt,
      f,
      halfAnnulus(dc, c, M.inner, M.inner - 0.015, from, M.facets),
      l0,
      l1,
      s.glow(hue),
    );
  // The bed, its pad and pillow, on its pedestal with the trim band.
  const [p0, p1] = M.pedestal;
  const [b0, b1] = M.bed;
  const [a0, a1] = M.bedA;
  k.bevelBox(p0, p1, dc - 0.15, dc + 0.15, 0, b0, 0.03, s.dark);
  k.box(
    p0 - 0.02,
    p1 + 0.02,
    dc - 0.17,
    dc + 0.17,
    b0 - 0.1,
    b0 - 0.05,
    trim(s.dark, accent2),
  );
  k.bevelBox(a0, a1, dc - 0.16, dc + 0.16, b0, b1, 0.015, s.body);
  k.bevelBox(
    a0 + 0.03,
    a1 - 0.03,
    dc - 0.14,
    dc + 0.14,
    b1,
    M.pad,
    0.025,
    s.panel,
  );
  k.bevelBox(
    a0 + 0.06,
    a0 + 0.32,
    dc - 0.11,
    dc + 0.11,
    M.pad,
    M.pad + 0.05,
    0.02,
    s.panel,
  );
  // The vitals monitor.
  const [m0, m1] = M.monitor;
  k.bevelBox(-0.85, -0.45, 0, 0.08, m0, m1, 0.015, s.dark);
  k.panel(
    -0.82,
    -0.48,
    0.08 + DECAL_LIFT,
    m0 + 0.03,
    m1 - 0.03,
    s.glow(ctx.look.palette.screenText),
  );
}

/** The treatment chair: its pedestal, seat, back, leg rest and lamp arm. */
const TREATMENT_CHAIR = {
  seat: [0.42, 0.5],
  seatA: [-0.3, 0.25],
  d: [0.22, 0.68],
  back: { from: [-0.3, 0.55], to: [-0.8, 1.15] },
  legs: { from: [0.25, 0.52], to: [0.72, 0.28] },
  arm: 2.0,
  lamp: { a: 0.0, d: 0.5 },
} as const;

/**
 * Med scanner variant 2, a treatment chair: a reclined padded chair on a
 * round foot and a column, its seat, back and leg rest each a padded slab
 * with a headrest and a foot plate, armrests on posts, and an overhead
 * arm from a wall post holding a lamp whose lens glows over the chair.
 * The column's trim band carries the tag's second accent.
 */
function treatmentChair({ k, s, ctx, out, accent2 }: Recipe) {
  const T = TREATMENT_CHAIR;
  const dc = out / 2;
  const [s0, s1] = T.seat;
  const [sa0, sa1] = T.seatA;
  const [d0, d1] = T.d;
  k.cylinder(-0.05, dc, 0, 0.08, 0.3, 16, s.dark);
  k.bevelBox(-0.17, 0.07, dc - 0.12, dc + 0.12, 0.08, s0, 0.02, s.body);
  k.box(
    -0.19,
    0.09,
    dc - 0.14,
    dc + 0.14,
    s0 - 0.1,
    s0 - 0.06,
    trim(s.dark, accent2),
  );
  k.bevelBox(sa0, sa1, d0, d1, s0, s1, 0.02, s.body);
  k.bevelBox(sa0, sa1, d0 + 0.02, d1 - 0.02, s1, s1 + 0.07, 0.03, s.panel);
  // The back and the leg rest: tilted padded slabs across the chair.
  const slab = (
    from: readonly number[],
    to: readonly number[],
    thick: number,
  ) => {
    const [fa = 0, fh = 0] = from;
    const [ta = 0, th = 0] = to;
    const angle = Math.atan2(th - fh, ta - fa);
    const bar = tiltedBar(
      (fa + ta) / 2,
      (fh + th) / 2,
      angle,
      Math.hypot(ta - fa, th - fh),
      thick,
    );
    k.extrude(bar, d0 + 0.02, d1 - 0.02, s.panel);
  };
  slab(T.back.from, T.back.to, 0.1);
  slab(T.legs.from, T.legs.to, 0.08);
  const [ba, bh] = T.back.to;
  k.bevelBox(
    ba - 0.08,
    ba + 0.1,
    dc - 0.12,
    dc + 0.12,
    bh - 0.08,
    bh + 0.08,
    0.03,
    s.panel,
  );
  const [la, lh] = T.legs.to;
  k.box(
    la - 0.02,
    la + 0.04,
    d0 + 0.04,
    d1 - 0.04,
    lh - 0.12,
    lh + 0.02,
    s.dark,
  );
  // The armrests on their posts.
  for (const dd of [d0 - 0.04, d1])
    k.box(-0.25, 0.15, dd, dd + 0.04, 0.72, 0.76, s.dark);
  for (const dd of [d0 - 0.04, d1])
    k.box(-0.07, -0.03, dd, dd + 0.04, s1, 0.72, s.metal);
  // The overhead arm from the wall post, and the lamp over the chair.
  const L = T.lamp;
  k.bevelBox(-0.5, -0.38, 0, 0.08, 1.2, T.arm + 0.12, 0.015, s.dark);
  k.box(-0.47, -0.41, 0.08, L.d + 0.03, T.arm, T.arm + 0.06, s.metal);
  k.box(
    -0.47,
    L.a + 0.03,
    L.d - 0.03,
    L.d + 0.03,
    T.arm - 0.06,
    T.arm,
    s.metal,
  );
  k.cylinder(L.a, L.d, T.arm - 0.12, T.arm - 0.06, 0.02, 8, s.metal);
  k.lathe(
    L.a,
    L.d,
    [
      [0, T.arm - 0.3],
      [0.17, T.arm - 0.3],
      [0.17, T.arm - 0.26],
      [0.06, T.arm - 0.12],
      [0, T.arm - 0.12],
    ],
    14,
    s.dark,
  );
  k.cylinder(
    L.a,
    L.d,
    T.arm - 0.31,
    T.arm - 0.3,
    0.14,
    14,
    s.glow(ctx.look.palette.lamp),
  );
}

/** The glass case: the plinth, the posts, the panes, the core, the cap. */
const GLASS_CASE = {
  plinth: 0.3,
  half: 0.35,
  post: 0.06,
  panes: [0.6, 1.4],
  cap: [1.7, 1.9],
  core: 0.2,
  coreAt: 1.0,
  emitter: 0.14,
  beam: 0.06,
} as const;

/**
 * Containment variant 1, a glass case: a square plinth, four corner posts
 * holding four tinted panes that stop short of the plinth and the cap, a
 * core glowing in a beam between two emitter dishes (held on thin rods,
 * the beam shows as bright bands in the gaps), and a square cap with a
 * port and a conduit to the wall. The ring round the cap's port carries
 * the tag's second accent.
 */
function glassCase({ k, s, hue, out, accent2 }: Recipe) {
  const G = GLASS_CASE;
  const d = out / 2;
  const x = G.half;
  const w = G.post / 2;
  const [c0, c1] = G.cap;
  k.bevelBox(
    -x - 0.1,
    x + 0.1,
    d - x - 0.08,
    d + x + 0.08,
    0,
    G.plinth,
    0.03,
    s.body,
  );
  for (const a of [-x, x])
    for (const dd of [d - x, d + x])
      k.box(a - w, a + w, dd - w, dd + w, G.plinth, c0, s.dark);
  // The panes, each between two posts.
  const glass = s.glow(shade(hue, 0.3));
  const [g0, g1] = G.panes;
  const t = 0.005;
  for (const dd of [d - x, d + x])
    k.box(-x + w, x - w, dd - t, dd + t, g0, g1, glass);
  for (const a of [-x, x])
    k.box(a - t, a + t, d - x + w, d + x - w, g0, g1, glass);
  // The emitters, the rods, the beam and the core.
  const e = G.emitter;
  k.lathe(
    0,
    d,
    [
      [0, G.plinth],
      [e, G.plinth],
      [e, G.plinth + 0.03],
      [0.04, G.plinth + 0.1],
      [0, G.plinth + 0.1],
    ],
    12,
    s.dark,
  );
  k.lathe(
    0,
    d,
    [
      [0, c0 - 0.1],
      [0.04, c0 - 0.1],
      [e, c0 - 0.03],
      [e, c0],
      [0, c0],
    ],
    12,
    s.dark,
  );
  k.cylinder(0, d, G.plinth + 0.1, c0 - 0.1, 0.012, 6, s.metal);
  k.cylinder(0, d, G.plinth + 0.1, c0 - 0.1, G.beam, 10, s.glow(hue));
  const core: [number, number][] = [];
  for (let i = 0; i <= 8; i++) {
    const a = Math.PI * (i / 8 - 0.5);
    core.push([G.core * Math.cos(a), G.coreAt + G.core * Math.sin(a)]);
  }
  k.lathe(0, d, core, 12, s.glow(hue));
  // The cap, its port and ring, and the conduit.
  k.bevelBox(
    -x - 0.07,
    x + 0.07,
    d - x - 0.07,
    d + x + 0.07,
    c0,
    c1,
    0.03,
    s.body,
  );
  k.cylinder(0, d, c1, c1 + 0.06, 0.14, 12, s.dark);
  k.ring(0, d, c1 + 0.03, 0.15, 0.03, 6, 12, trim(s.metal, accent2), "up");
  k.box(-0.08, 0.08, 0, d - x - 0.07, c0 + 0.05, c0 + 0.15, s.dark);
}

/** The twin cells: the shared base, each tank's place and parts. */
const TWIN_CELLS = {
  base: 0.28,
  at: 0.42,
  radius: 0.26,
  glass: 0.22,
  core: 0.14,
  tank: [0.33, 1.47],
  glassSpan: [0.4, 1.4],
  cap: [1.52, 1.7],
  ribs: [0.6, 0.9, 1.2],
  header: 1.62,
} as const;

/**
 * Containment variant 2, twin cells: one long base carrying two smaller
 * tanks side by side, each a tinted glass shell round its own glowing
 * core held by ribs, under its own cap, a header pipe joining the caps
 * and a conduit to the wall, and a status panel glowing on the base's
 * front. Each cap's ring carries the tag's second accent.
 */
function twinCells({ k, s, ctx, hue, half, out, accent2 }: Recipe) {
  const T = TWIN_CELLS;
  const d = out / 2;
  const [t0, t1] = T.tank;
  const [c0, c1] = T.cap;
  k.bevelBox(
    -half + 0.05,
    half - 0.05,
    0.05,
    out - 0.05,
    0,
    T.base,
    0.03,
    s.body,
  );
  for (const a of [-T.at, T.at]) {
    k.cylinder(a, d, T.base, c0, T.core, 12, s.glow(hue));
    const [g0, g1] = T.glassSpan;
    k.lathe(
      a,
      d,
      [
        [0, g0],
        [T.glass, g0],
        [T.glass, g1],
        [0, g1],
      ],
      14,
      s.glow(shade(hue, 0.3)),
    );
    k.cylinder(a, d, T.base, t0, T.radius, 14, s.dark);
    for (const h of T.ribs)
      k.ring(a, d, h, T.glass + 0.02, 0.022, 6, 14, s.metal, "up");
    k.cylinder(a, d, t1, c0, T.radius, 14, s.dark);
    k.cylinder(a, d, c0, c1, T.radius, 14, s.body);
    k.cylinder(a, d, c1, c1 + 0.05, T.radius - 0.1, 12, trim(s.dark, accent2));
  }
  k.cylinderAlong(-T.at, T.at, d, T.header, 0.06, 10, s.metal);
  k.box(-0.08, 0.08, 0, d, T.header - 0.05, T.header + 0.05, s.dark);
  k.panel(
    -0.15,
    0.15,
    out - 0.05 + DECAL_LIFT,
    0.1,
    0.2,
    s.glow(ctx.look.palette.screenText),
  );
}

/**
 * The recipes of every machine kind, one per variant (2.7 C2): entry 0 is
 * variant 0, the kind's first model (2.7 C1), and `VARIANT_COUNTS.machine` says how many each
 * kind draws.
 */
const RECIPES: Record<MachineKind, readonly ((r: Recipe) => void)[]> = {
  workbench: [workbench, fitterBench, weldingBench],
  "lab-bench": [labBench, wetBench, analysisBench],
  "server-rack": [serverRack, twinRacks, openRack],
  "cryo-pod": [cryoPod, leaningPod, drumPod],
  fabricator: [fabricator, resinPrinter, armCell],
  hydroponics: [hydroponics, trayRack, tubeGarden],
  "nav-table": [navTable, plottingTable, chartLectern],
  "comms-array": [commsArray, latticeMast, radioRack],
  "reactor-coupling": [reactorCoupling, lyingCoupling, sphereVessel],
  "cargo-loader": [cargoLoader, scissorLift, gantryHoist],
  "med-scanner": [medScanner, ringScanner, treatmentChair],
  containment: [containment, glassCase, twinCells],
};
