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
import { FOOTPRINTS } from "../../world/move";
import type { Fixture, MachineKind } from "../../world/types";
import type { Frame, Kit } from "../kit";
import { frameForSlot } from "../kit";
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

/** Four square legs under a top spanning `a0..a1`, `d0..d1`, up to `h`. */
function legs(
  k: Kit,
  a0: number,
  a1: number,
  d0: number,
  d1: number,
  h: number,
  s: Parameters<Kit["box"]>[6],
) {
  const w = 0.05;
  for (const a of [a0, a1 - w]) {
    for (const d of [d0, d1 - w]) k.box(a, a + w, d, d + w, 0, h, s);
  }
}

/**
 * Workbench: a steel bench on legs with a lower shelf, a vice clamped to
 * its front corner, a pegboard tool rack on the wall with tools hanging
 * from it, and a lamp on an arm whose bulb glows under its shade.
 */
function workbench({ k, s, ctx, rng, half, out }: Recipe) {
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const top = 0.9;
  k.bevelBox(a0, a1, 0.02, out - 0.1, top - 0.06, top, 0.012, s.body);
  legs(k, a0 + 0.04, a1 - 0.04, 0.05, out - 0.14, top - 0.06, s.metal);
  k.box(a0 + 0.06, a1 - 0.06, 0.08, out - 0.16, 0.18, 0.22, s.metal);
  // The vice: base, fixed jaw, moving jaw and the screw's tommy bar.
  const v = a1 - 0.3;
  k.bevelBox(v, v + 0.2, out - 0.3, out - 0.12, top, top + 0.05, 0.01, s.dark);
  k.box(
    v + 0.03,
    v + 0.17,
    out - 0.3,
    out - 0.25,
    top + 0.05,
    top + 0.13,
    s.dark,
  );
  k.box(
    v + 0.03,
    v + 0.17,
    out - 0.19,
    out - 0.14,
    top + 0.05,
    top + 0.13,
    s.dark,
  );
  k.cylinderAlong(v - 0.02, v + 0.22, out - 0.12, top + 0.09, 0.01, 6, s.metal);
  // The pegboard and its tools, their lengths from the seed.
  k.bevelBox(
    a0 + 0.05,
    0.35,
    0,
    0.03,
    1.1,
    1.8,
    0.01,
    s.tinted(shade(ctx.look.palette.metal, 0.7), LAYER.metal),
  );
  for (let i = 0; i < 6; i++) {
    const a = a0 + 0.15 + i * 0.17;
    const len = rng.range(0.18, 0.4);
    k.box(a, a + 0.03, 0.03, 0.05, 1.7 - len, 1.7, s.metal);
    k.box(a - 0.02, a + 0.05, 0.03, 0.055, 1.66, 1.72, s.dark);
  }
  // The lamp: foot, post, arm, shade and the glowing bulb under it.
  const [la, ld] = [a1 - 0.12, 0.14];
  k.cylinder(la, ld, top, top + 0.03, 0.07, 10, s.dark);
  k.box(
    la - 0.015,
    la + 0.015,
    ld - 0.015,
    ld + 0.015,
    top + 0.03,
    1.45,
    s.metal,
  );
  k.box(la - 0.015, la + 0.015, ld - 0.015, 0.5, 1.42, 1.45, s.metal);
  k.lathe(
    la,
    0.5,
    [
      [0, 1.3],
      [0.1, 1.3],
      [0.04, 1.42],
      [0, 1.44],
    ],
    10,
    s.dark,
  );
  k.cylinder(la, 0.5, 1.285, 1.3, 0.075, 10, s.glow(ctx.look.palette.lamp));
}

/**
 * Lab bench: a cabinet bench with a resin top, two glass flasks glowing
 * with what they hold, a rack of test tubes and a shelf of reagent
 * bottles on the wall.
 */
function labBench({ k, s, ctx, hue, rng, half, out }: Recipe) {
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const top = 0.94;
  k.bevelBox(
    a0 + 0.02,
    a1 - 0.02,
    0.02,
    out - 0.18,
    0,
    top - 0.05,
    0.02,
    s.body,
  );
  for (let i = 0; i < 3; i++) {
    const c = a0 + 0.1 + i * ((a1 - a0 - 0.2) / 3);
    k.box(c, c + 0.5, out - 0.18, out - 0.17, 0.1, top - 0.15, s.panel);
    k.box(
      c + 0.2,
      c + 0.3,
      out - 0.17,
      out - 0.15,
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
    top - 0.05,
    top,
    0.01,
    s.tinted(shade(ctx.look.palette.metal, 0.4)),
  );
  // Two flasks: a conical one and a round-bottomed one on a ring stand.
  const liquid = s.glow(hueToRgb(rng.range(0, 360), 0.8, 0.6));
  k.lathe(
    -0.45,
    0.45,
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
  k.cylinder(-0.1, 0.45, top, top + 0.12, 0.05, 8, s.dark);
  k.lathe(
    -0.1,
    0.45,
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
  // The test tube rack: base, top bar, end posts and six tubes.
  const [r0, r1] = [0.25, 0.65];
  k.box(r0, r1, 0.3, 0.42, top, top + 0.02, s.dark);
  k.box(r0, r1, 0.3, 0.42, top + 0.1, top + 0.12, s.dark);
  k.box(r0, r0 + 0.02, 0.3, 0.42, top, top + 0.12, s.dark);
  k.box(r1 - 0.02, r1, 0.3, 0.42, top, top + 0.12, s.dark);
  for (let i = 0; i < 6; i++) {
    const a = r0 + 0.06 + i * 0.056;
    k.cylinder(
      a,
      0.36,
      top + 0.02,
      top + rng.range(0.16, 0.2),
      0.012,
      6,
      s.tinted(shade(hue, 0.9)),
    );
  }
  // The reagent shelf and its bottles.
  k.box(a0 + 0.1, a1 - 0.1, 0, 0.22, 1.5, 1.53, s.metal);
  for (let i = 0; i < 7; i++) {
    const a = a0 + 0.2 + i * 0.2;
    k.cylinder(
      a,
      0.12,
      1.53,
      1.53 + rng.range(0.12, 0.22),
      0.04,
      8,
      s.tinted(hueToRgb(rng.range(0, 360), 0.5, 0.4)),
    );
  }
}

/**
 * Server rack: a tall cabinet with ten unit slabs, each with a row of LED
 * dots, a vented top and a cable duct to the wall.
 */
function serverRack({ k, s, ctx, hue, rng, half, out }: Recipe) {
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const [d1, top] = [out - 0.05, 2.2];
  k.bevelBox(a0, a1, 0.02, d1, 0, top, 0.025, s.body);
  k.box(-0.15, 0.15, 0, 0.02, top - 0.3, top - 0.1, s.dark);
  const leds: Rgb[] = [
    hue,
    ctx.look.palette.screenText,
    hueToRgb(35, 0.9, 0.55),
  ];
  for (let u = 0; u < 10; u++) {
    const h0 = 0.14 + u * 0.19;
    k.bevelBox(
      a0 + 0.04,
      a1 - 0.04,
      d1,
      d1 + 0.02,
      h0,
      h0 + 0.16,
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
    for (let i = 0; i < 3; i++) {
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
  for (let i = 0; i < 5; i++) {
    const a = a0 + 0.1 + i * 0.12;
    k.box(a, a + 0.06, 0.1, d1 - 0.1, top, top + 0.02, s.dark);
  }
}

/**
 * Cryo pod: an upright capsule on a round plinth with a collar ring, a
 * framed glass front glowing faintly with the cold inside, and feed lines
 * to the wall.
 */
function cryoPod({ k, s, ctx, out }: Recipe) {
  const d = out / 2;
  const r = Math.min(0.42, d - 0.06);
  k.cylinder(0, d, 0, 0.15, r + 0.03, 14, s.dark);
  k.ring(0, d, 0.16, r, 0.04, 6, 16, s.metal, "up");
  k.lathe(
    0,
    d,
    [
      [0, 0.15],
      [r - 0.06, 0.15],
      [r, 0.3],
      [r, 1.75],
      [r - 0.08, 1.95],
      [0.15, 2.05],
      [0, 2.07],
    ],
    14,
    s.body,
  );
  // The window frame and the glass in it.
  const front = d + r * Math.cos(Math.PI / 14) - 0.02;
  k.bevelBox(
    -0.28,
    0.28,
    front - 0.12,
    front + 0.03,
    0.45,
    1.75,
    0.02,
    s.metal,
  );
  k.panel(
    -0.22,
    0.22,
    front + 0.031,
    0.52,
    1.68,
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

/**
 * Fabricator: a squat box with an inner chamber open at the front, lit
 * from the top inside, a print head on a nozzle arm running on a rail,
 * a half printed part on the bed and a control screen on the side.
 */
function fabricator({ k, s, hue, half, out }: Recipe) {
  const [a0, a1] = [-half + 0.05, half - 0.05];
  const d1 = out - 0.05;
  const [c0, c1] = [0.6, 1.3];
  const [side0, side1] = [a0 + 0.3, a1 - 0.3];
  k.bevelBox(a0, a1, 0.02, d1, 0, c0, 0.03, s.body);
  k.bevelBox(a0, a1, 0.02, d1, c1, 1.6, 0.03, s.body);
  k.box(a0, side0, 0.02, d1, c0, c1, s.body);
  k.box(side1, a1, 0.02, d1, c0, c1, s.body);
  k.box(side0, side1, 0.02, 0.35, c0, c1, s.dark);
  // The bed, the glowing chamber light and the printed part.
  k.box(side0 + 0.05, side1 - 0.05, 0.35, d1 - 0.05, c0, c0 + 0.03, s.metal);
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
  k.cylinderAlong(side0, side1, 0.6, c1 - 0.06, 0.02, 8, s.metal);
  k.box(-0.06, 0.06, 0.55, 0.65, c1 - 0.12, c1 - 0.03, s.dark);
  k.cylinder(0, 0.6, c0 + 0.25, c1 - 0.12, 0.025, 8, s.metal);
  k.cylinder(0, 0.6, c0 + 0.18, c0 + 0.25, 0.012, 6, s.dark);
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

/**
 * Hydroponics: a long trough on legs with three domes of foliage growing
 * out of it and a grow-light bar glowing magenta on arms from the wall.
 */
function hydroponics({ k, s, half, out }: Recipe) {
  const [a0, a1] = [-half + 0.02, half - 0.02];
  const [t0, t1] = [0.55, 0.8];
  const dc = out / 2 - 0.05;
  k.bevelBox(a0, a1, dc - 0.3, dc + 0.3, t0, t1, 0.02, s.body);
  legs(k, a0 + 0.06, a1 - 0.06, dc - 0.26, dc + 0.26, t0, s.metal);
  k.box(
    a0 + 0.04,
    a1 - 0.04,
    dc - 0.26,
    dc + 0.26,
    t1,
    t1 + 0.01,
    s.tinted(hueToRgb(30, 0.4, 0.2)),
  );
  const leaf = s.tinted(hueToRgb(115, 0.5, 0.35));
  for (const a of [-0.55, 0, 0.55]) {
    k.lathe(
      a,
      dc,
      [
        [0, t1],
        [0.25, t1],
        [0.24, t1 + 0.08],
        [0.2, t1 + 0.17],
        [0.12, t1 + 0.23],
        [0, t1 + 0.25],
      ],
      10,
      leaf,
    );
  }
  // The grow-light bar on two arms.
  for (const a of [a0 + 0.1, a1 - 0.14])
    k.box(a, a + 0.04, 0, dc + 0.05, 1.5, 1.54, s.metal);
  k.box(
    a0 + 0.05,
    a1 - 0.05,
    dc - 0.05,
    dc + 0.05,
    1.46,
    1.5,
    s.glow(hueToRgb(300, 0.8, 0.6)),
  );
}

/**
 * Nav table: a pedestal with a slanted top tilted towards the navigator,
 * a glowing star map inset into the slope and a rim of controls.
 */
function navTable({ k, kitAt, f, s, ctx, half, out }: Recipe) {
  const [a0, a1] = [-half + 0.05, half - 0.05];
  k.bevelBox(-0.55, 0.55, 0.2, 0.7, 0, 0.78, 0.03, s.body);
  k.bevelBox(-0.7, 0.7, 0.15, 0.75, 0, 0.08, 0.02, s.dark);
  // The slanted top, high at the back, low at the front.
  const [back, front] = [0.05, out - 0.05];
  const [hb, hf] = [1.05, 0.86];
  const at = (d: number) => hb + ((d - back) / (front - back)) * (hf - hb);
  profileAlong(
    kitAt,
    f,
    [
      [back, 0.78],
      [front, 0.78],
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
  for (let i = 0; i < 8; i++) {
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

/**
 * Comms array: a narrow mast on an equipment base, a dish turned to the
 * sky with its feed horn and a blinking tip, and a pair of whip antennas.
 */
function commsArray({ k, s, hue, out }: Recipe) {
  const d = out / 2;
  k.bevelBox(-0.3, 0.3, 0.03, out - 0.03, 0, 0.4, 0.03, s.body);
  k.box(-0.2, 0.0, out - 0.03, out - 0.025, 0.2, 0.3, s.glow(shade(hue, 0.9)));
  k.cylinder(0, d, 0.4, 2.0, 0.045, 8, s.metal);
  k.ring(0, d, 1.2, 0.08, 0.02, 6, 10, s.dark, "up");
  const r = Math.min(0.28, d - 0.02);
  k.lathe(
    0,
    d,
    [
      [0, 1.98],
      [r, 2.1],
      [r, 2.13],
      [0, 2.02],
    ],
    16,
    s.panel,
  );
  k.cylinder(0, d, 2.02, 2.3, 0.012, 6, s.metal);
  k.cylinder(0, d, 2.3, 2.36, 0.03, 8, s.glow(hueToRgb(0, 0.9, 0.55)));
  for (const a of [-0.45, 0.45]) {
    k.box(a - 0.02, a + 0.02, d - 0.02, d + 0.02, 0.4, 0.44, s.dark);
    k.cylinder(a, d, 0.44, 1.6, 0.008, 5, s.metal);
  }
  k.box(-0.45, 0.45, d - 0.02, d + 0.02, 0.38, 0.42, s.dark);
}

/**
 * Reactor coupling: two thick cylinders stacked and joined by collar
 * rings around a glowing core band, braced by side pipes with clamps and
 * a conduit to the wall.
 */
function reactorCoupling({ k, s, hue, half, out }: Recipe) {
  const d = out / 2;
  const r = Math.min(0.4, d - 0.05);
  k.cylinder(0, d, 0, 0.8, r, 16, s.body);
  k.cylinder(0, d, 0.8, 1.0, r - 0.1, 16, s.glow(hue));
  k.cylinder(0, d, 1.0, 1.9, r, 16, s.body);
  k.ring(0, d, 0.8, r - 0.06, 0.05, 6, 16, s.metal, "up");
  k.ring(0, d, 1.0, r - 0.06, 0.05, 6, 16, s.metal, "up");
  k.ring(0, d, 1.6, r, 0.03, 6, 16, s.dark, "up");
  const side = Math.min(0.7, half - 0.1);
  for (const a of [-side, side]) {
    k.cylinder(a, d, 0, 1.9, 0.07, 8, s.metal);
    for (const h of [0.4, 1.4]) {
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
  k.box(-0.1, 0.1, 0, d, 1.7, 1.8, s.dark);
}

/**
 * Cargo loader: a mast frame with a carriage and two forks carrying a
 * crate, hydraulic rams beside the mast and a warning beacon on top.
 */
function cargoLoader({ k, s, half, out }: Recipe) {
  const m = Math.min(0.75, half - 0.05);
  k.bevelBox(-m - 0.1, m + 0.1, 0, 0.25, 0, 0.12, 0.02, s.dark);
  for (const a of [-m, m - 0.1])
    k.bevelBox(a, a + 0.1, 0.05, 0.2, 0.12, 1.8, 0.015, s.body);
  k.bevelBox(-m, m, 0.05, 0.2, 1.7, 1.8, 0.015, s.body);
  // The carriage and its forks.
  k.box(-m + 0.1, m - 0.1, 0.2, 0.26, 0.3, 0.45, s.metal);
  for (const a of [-0.45, 0.3]) {
    k.box(a, a + 0.15, 0.2, 0.26, 0.3, 0.8, s.metal);
    k.box(a, a + 0.15, 0.26, out - 0.02, 0.3, 0.35, s.metal);
  }
  k.bevelBox(
    -0.55,
    0.55,
    0.3,
    out - 0.04,
    0.35,
    0.85,
    0.02,
    s.tinted(hueToRgb(30, 0.55, 0.45)),
  );
  k.box(-0.56, 0.56, 0.3, out - 0.04, 0.55, 0.6, s.hazard);
  // Hydraulic rams and the beacon.
  for (const a of [-m + 0.2, m - 0.2])
    k.cylinder(a, 0.12, 0.12, 1.2, 0.03, 8, s.metal);
  k.cylinder(
    m - 0.05,
    0.12,
    1.8,
    1.9,
    0.05,
    10,
    s.glow(hueToRgb(38, 0.95, 0.55)),
  );
}

/**
 * Med scanner: a padded bed on a pedestal, a half-ring arch standing over
 * it on rails with a glowing scan bar under its crown, and a vitals
 * monitor on the wall.
 */
function medScanner({ k, kitAt, f, s, ctx, hue, half, out }: Recipe) {
  const [a0, a1] = [-half + 0.05, half - 0.05];
  const dc = out / 2;
  const bed = 0.62;
  k.bevelBox(-0.4, 0.4, dc - 0.2, dc + 0.2, 0, bed - 0.08, 0.03, s.dark);
  k.bevelBox(a0, a1, dc - 0.35, dc + 0.35, bed - 0.08, bed, 0.02, s.body);
  k.bevelBox(
    a0 + 0.05,
    a1 - 0.05,
    dc - 0.31,
    dc + 0.31,
    bed,
    bed + 0.07,
    0.03,
    s.panel,
  );
  k.bevelBox(
    a0 + 0.1,
    a0 + 0.4,
    dc - 0.2,
    dc + 0.2,
    bed + 0.07,
    bed + 0.12,
    0.025,
    s.panel,
  );
  // The rails along both sides of the bed, and the arch on them.
  const [ro, ri] = [dc - 0.02, dc - 0.12];
  for (const d of [dc - ro, dc + ro - 0.08])
    k.box(a0, a1, d, d + 0.08, bed - 0.06, bed - 0.02, s.metal);
  const arch: [number, number][] = [];
  const n = 10;
  for (let i = 0; i <= n; i++) {
    const t = (Math.PI * i) / n;
    arch.push([dc - Math.cos(t) * ro, bed - 0.04 + Math.sin(t) * ro]);
  }
  for (let i = n; i >= 0; i--) {
    const t = (Math.PI * i) / n;
    arch.push([dc - Math.cos(t) * ri, bed - 0.04 + Math.sin(t) * ri]);
  }
  profileAlong(kitAt, f, arch, 0.05, 0.2, s.body);
  // The scan bar under the crown of the arch.
  const crown = bed - 0.04 + ri;
  k.box(
    0.08,
    0.17,
    dc - 0.3,
    dc + 0.3,
    crown - 0.03,
    crown + 0.005,
    s.glow(hue),
  );
  // The vitals monitor.
  k.bevelBox(a0 + 0.05, a0 + 0.45, 0, 0.08, 1.2, 1.5, 0.015, s.dark);
  k.panel(
    a0 + 0.08,
    a0 + 0.42,
    0.081,
    1.23,
    1.47,
    s.glow(ctx.look.palette.screenText),
  );
}

/**
 * Containment: a tank on a heavy base holding a glowing core, held by
 * ribs (rings round the core), vertical struts and a cap with a conduit
 * running to the wall.
 */
function containment({ k, s, hue, out }: Recipe) {
  const d = out / 2;
  const r = Math.min(0.4, d - 0.03);
  k.cylinder(0, d, 0, 0.3, r, 16, s.body);
  k.cylinder(0, d, 0.3, 1.7, r - 0.1, 16, s.glow(hue));
  for (const h of [0.55, 0.85, 1.15, 1.45])
    k.ring(0, d, h, r - 0.08, 0.035, 6, 16, s.metal, "up");
  for (let i = 0; i < 4; i++) {
    const t = (Math.PI / 4) * (1 + 2 * i);
    const [a, dd] = [Math.sin(t) * (r - 0.04), d + Math.cos(t) * (r - 0.04)];
    k.box(a - 0.02, a + 0.02, dd - 0.02, dd + 0.02, 0.3, 1.7, s.dark);
  }
  k.cylinder(0, d, 1.7, 1.95, r, 16, s.body);
  k.cylinder(0, d, 1.95, 2.0, r - 0.12, 12, s.dark);
  k.box(-0.08, 0.08, 0, d, 1.8, 1.9, s.dark);
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
