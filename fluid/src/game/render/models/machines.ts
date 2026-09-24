/**
 * The machines: one per tag, the kind picked by the tag's hash, so the
 * same tag looks the same in every room.
 *
 * Every machine stands against its wall slot inside its `FOOTPRINTS` size,
 * with the tag strip and the tag's label on the wall above it. Each recipe
 * below says what it depicts. Every part that glows (a lamp, LEDs, a core,
 * a grow light) sits on or in the machine's body, never in mid-air.
 */

import { createRng, type Rng } from "../../core/seed";
import { FOOTPRINTS } from "../../world/footprints";
import type { Fixture, MachineKind } from "../../world/types";
import type { Surface } from "../geometry";
import { frameForSlot, type Frame, type Kit } from "../kit";
import { LAYER } from "../layers";
import { hueToRgb, type Rgb } from "../looks";
import {
  profileAlong,
  shade,
  surfaces,
  type KitAt,
  type ModelContext,
  type Surfaces,
} from "./common";
import { tagStrip } from "./wall";

type Machine = Extract<Fixture, { kind: "machine" }>;

/** What a recipe gets: its kits, the look's surfaces and the tag's colour. */
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
}

/** Builds a machine and its tag strip against its wall slot. */
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
  RECIPES[fx.machine]({
    k,
    kitAt,
    f,
    s: surfaces(ctx.look),
    ctx,
    hue,
    rng: createRng(fx.seed),
    half: size.along / 2,
    out: size.out,
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
function workbench({ k, s, ctx, rng, half, out }: Recipe) {
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
  k.box(v + 0.03, v + W.vice - 0.03, out - 0.3, out - 0.25, j0, j1, s.dark);
  k.box(v + 0.03, v + W.vice - 0.03, out - 0.19, out - 0.14, j0, j1, s.dark);
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
function labBench({ k, s, ctx, hue, rng, half, out }: Recipe) {
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
      s.dark,
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
function serverRack({ k, s, ctx, hue, rng, half, out }: Recipe) {
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
    k.box(a, a + 0.06, 0.1, d1 - 0.1, top, top + 0.02, s.dark);
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
function cryoPod({ k, s, ctx, out }: Recipe) {
  const C = CRYO_POD;
  const d = out / 2;
  const r = Math.min(C.radius, d - 0.06);
  k.cylinder(0, d, 0, C.plinth, r + 0.03, C.sides, s.dark);
  k.ring(0, d, C.plinth + 0.01, r, 0.04, 6, 16, s.metal, "up");
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
    front + 0.031,
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
function fabricator({ k, s, hue, half, out }: Recipe) {
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
    s.dark,
  );
  k.cylinder(0, F.rail, c0 + 0.25, c1 - 0.12, 0.025, 8, s.metal);
  k.cylinder(0, F.rail, c0 + 0.18, c0 + 0.25, 0.012, 6, s.dark);
  // A control screen on the right side block.
  k.panel(
    side1 + 0.04,
    a1 - 0.04,
    d1 + 0.001,
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
function hydroponics({ k, s, half, out }: Recipe) {
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
    k.box(a, a + 0.04, 0, dc + 0.05, H.light, H.light + 0.04, s.metal);
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
function navTable({ k, kitAt, f, s, ctx, half, out }: Recipe) {
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
      i % 3 === 0 ? s.metal : s.dark,
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
function commsArray({ k, s, hue, out }: Recipe) {
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
    s.dark,
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
function reactorCoupling({ k, s, hue, half, out }: Recipe) {
  const R = REACTOR;
  const d = out / 2;
  const r = Math.min(R.radius, d - 0.05);
  const [c0, c1] = R.core;
  k.cylinder(0, d, 0, c0, r, 16, s.body);
  k.cylinder(0, d, c0, c1, r - 0.1, 16, s.glow(hue));
  k.cylinder(0, d, c1, R.top, r, 16, s.body);
  k.ring(0, d, c0, r - 0.06, 0.05, 6, 16, s.metal, "up");
  k.ring(0, d, c1, r - 0.06, 0.05, 6, 16, s.metal, "up");
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
function cargoLoader({ k, s, half, out }: Recipe) {
  const C = CARGO_LOADER;
  const m = Math.min(0.75, half - 0.05);
  k.bevelBox(-m - 0.1, m + 0.1, 0, 0.25, 0, C.base, 0.02, s.dark);
  for (const a of [-m, m - 0.1])
    k.bevelBox(a, a + 0.1, 0.05, 0.2, C.base, C.mast, 0.015, s.body);
  k.bevelBox(-m, m, 0.05, 0.2, C.mast - 0.1, C.mast, 0.015, s.body);
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
function medScanner({ k, kitAt, f, s, ctx, hue, half, out }: Recipe) {
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
    k.box(a0, a1, d, d + 0.08, bed - 0.06, bed - 0.02, s.metal);
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
    0.081,
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
function containment({ k, s, hue, out }: Recipe) {
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
  k.cylinder(0, d, C.cap, C.cap + 0.05, r - 0.12, 12, s.dark);
  k.box(-0.08, 0.08, 0, d, C.cap - 0.15, C.cap - 0.05, s.dark);
}

/** The recipe of every machine kind. */
const RECIPES: Record<MachineKind, (r: Recipe) => void> = {
  workbench,
  "lab-bench": labBench,
  "server-rack": serverRack,
  "cryo-pod": cryoPod,
  fabricator,
  hydroponics,
  "nav-table": navTable,
  "comms-array": commsArray,
  "reactor-coupling": reactorCoupling,
  "cargo-loader": cargoLoader,
  "med-scanner": medScanner,
  containment,
};
