/**
 * The workshop heroes' recipes: the core wall, the gun rack, the gun
 * bench, the tube bench and the field pack. What they share is the
 * workbench and the wall rack: heavy benches with tools and cables (the
 * two benches), wall frames that hold a piece (the rack, the quilted core
 * wall), ribbed cylinders and hoses (the big gun, the pack) and glowing
 * cores, tubes and status lights.
 *
 * Two helpers carry the shared shapes: `bigGun` (the oversized energy
 * weapon, identical wherever it is called, only its placement box moves)
 * and `workbench` (the heavy desk under the gun bench and the tube bench:
 * a top slab, four legs, a lower shelf and a blank pegboard rising
 * `PEGBOARD_RISE` above the slab). A leaning or tilted part (a splayed
 * leg, a hinged panel, a jointed lamp arm) is always built with
 * `tiltedBar` inside `profileAlong`, in the frame's own `(d, h)` plane,
 * never with `sideways` alone.
 */

import type { HeroKind } from "../../../world/types";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import {
  profileAlong,
  tiltedBar,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import { heroHalf, type HeroRecipe } from "./common";
import { pixelPanel, textRows } from "./pixels";

/** The big gun's dark-grey body and the workbenches' metal frames. */
const GUN_METAL: Rgb = [0.16, 0.16, 0.18];

/** The big gun's core: a deep, glowing green. */
const CORE_GREEN: Rgb = [0.25, 1.0, 0.45];

/** The quilted core wall's padded cushions: a worn fabric grey. */
const PAD_TINT: Rgb = [0.4, 0.36, 0.32];

/** A status light that says all is well. */
const STATUS_GREEN: Rgb = [0.25, 1.0, 0.35];

/** A status light that says wait. */
const STATUS_AMBER: Rgb = [1.0, 0.6, 0.12];

/** A status light that says trouble. */
const STATUS_RED: Rgb = [1.0, 0.15, 0.1];

/** A row of status lights' colours, groups in order. */
const STATUS_TINTS: readonly Rgb[] = [
  STATUS_GREEN,
  STATUS_GREEN,
  STATUS_AMBER,
  STATUS_GREEN,
  STATUS_RED,
];

/** The tube bench's three tubes: amber, cyan and violet plasma. */
const TUBE_TINTS: readonly Rgb[] = [
  [1.0, 0.55, 0.15],
  [0.3, 0.8, 1.0],
  [0.85, 0.3, 1.0],
];

/** The soldering lamp's bulb: a warm white filament. */
const LAMP_WARM: Rgb = [1.0, 0.92, 0.75];

/** The field pack's shell: a bulky, utilitarian grey. */
const PACK_GREY: Rgb = [0.5, 0.51, 0.54];

/** The wand's tip: a bright status cyan. */
const WAND_CYAN: Rgb = [0.35, 0.95, 1.0];

/** The chamfer of a hero's housings, in metres. */
const HOUSING_BEVEL = 0.02;

// --- The big gun ---------------------------------------------------

/** The big gun's overall length, in metres. */
const GUN_LENGTH = 1.3;
/** The big gun's body depth (back to front), in metres. */
const GUN_DEPTH = 0.22;
/** The big gun's body height, in metres. */
const GUN_HEIGHT = 0.3;
/** How much of the gun's length is left clear for the barrel's muzzle. */
const MUZZLE_LEN = 0.22;
/** The barrel's radius, in metres. */
const MUZZLE_RADIUS = 0.1;
/** How far the two vent collars bulge past the barrel's radius. */
const VENT_BULGE = 0.015;
/** The grip's half-width, hanging below the body's back. */
const GRIP_WIDTH = 0.05;
/** How far the grip hangs below the body's belly. */
const GRIP_DROP = 0.16;
/** The glowing core ridge's thickness, standing on the body's back. */
const CORE_THICK = 0.03;

/**
 * The big green gun: an oversized dark-grey energy weapon lying along `a`,
 * a fixed shape wherever it is called (only its placement moves, so the
 * rack's and the bench's guns are the same part sizes). Its body runs
 * `GUN_LENGTH` long, centred between `a0` and `a1` rather than stretched
 * to fill them (so it never grows or shrinks between callers), `GUN_DEPTH`
 * deep from `min(d0, d1)` and `GUN_HEIGHT` tall from `h0`: a bevelled
 * block, a stepped barrel with two wider vent collars at the muzzle end, a
 * grip hanging off the back, and a glowing green core ridge standing on
 * its spine (blink group 0, the kind's breathe bank).
 */
export function bigGun(
  k: Kit,
  s: Surfaces,
  a0: number,
  a1: number,
  d0: number,
  d1: number,
  h0: number,
): void {
  const ac = (a0 + a1) / 2;
  const half = Math.min(GUN_LENGTH, Math.abs(a1 - a0) - 0.1) / 2;
  const aStart = ac - half;
  const aEnd = ac + half;
  const dBack = Math.min(d0, d1);
  const dFront = dBack + GUN_DEPTH;
  const hBottom = h0;
  const hTop = h0 + GUN_HEIGHT;
  const dc = (dBack + dFront) / 2;
  const hc = (hBottom + hTop) / 2;
  const metal = s.tinted(GUN_METAL);
  const muzzleStart = aEnd - MUZZLE_LEN;

  k.bevelBox(
    aStart,
    muzzleStart,
    dBack,
    dFront,
    hBottom,
    hTop,
    HOUSING_BEVEL,
    metal,
  );
  k.cylinderAlong(muzzleStart, aEnd, dc, hc, MUZZLE_RADIUS, 10, s.dark);
  for (const t of [0.3, 0.68]) {
    const av = muzzleStart + (aEnd - muzzleStart) * t;
    k.cylinderAlong(
      av - 0.015,
      av + 0.015,
      dc,
      hc,
      MUZZLE_RADIUS + VENT_BULGE,
      10,
      metal,
    );
  }
  k.bevelBox(
    aStart + 0.08,
    aStart + 0.08 + GRIP_WIDTH,
    dc - 0.05,
    dc + 0.05,
    hBottom - GRIP_DROP,
    hBottom,
    0.01,
    s.dark,
  );
  k.box(
    aStart + 0.14,
    muzzleStart - 0.05,
    dc - CORE_THICK / 2,
    dc + CORE_THICK / 2,
    hTop,
    hTop + CORE_THICK,
    s.blink(CORE_GREEN, 0),
  );
}

// --- The workbench ---------------------------------------------------

/** How far above the top slab the pegboard rises, in metres. */
const PEGBOARD_RISE = 0.5;

/** The thickness of the workbench's top slab. */
const SLAB_THICK = 0.05;

/** Both benches' top slab height (H8, the plan's table). */
export const WORKBENCH_TOP = 0.9;

/**
 * A heavy workbench, backed against its wall: a top slab from
 * `WORKBENCH_TOP - SLAB_THICK` to `top` over the whole footprint, four
 * short legs at its corners, a low shelf at a third height and a blank
 * pegboard panel standing at the wall from `top` up to
 * `top + PEGBOARD_RISE`. `hw` is the bench's half width and `depth` its
 * footprint depth (`heroHalf`'s `d1`); every recipe that calls it adds its
 * own gear on the slab and the board.
 */
export function workbench(
  k: Kit,
  s: Surfaces,
  hw: number,
  depth: number,
  top: number,
): void {
  k.bevelBox(-hw, hw, 0, depth, top - SLAB_THICK, top, 0.015, s.body);
  const legIn = 0.08;
  for (const a of [-hw + legIn, hw - legIn])
    for (const d of [depth * 0.1, depth * 0.9])
      k.box(
        a - 0.03,
        a + 0.03,
        d - 0.03,
        d + 0.03,
        0,
        top - SLAB_THICK,
        s.metal,
      );
  k.box(-hw + 0.1, hw - 0.1, depth * 0.15, depth * 0.85, 0.26, 0.31, s.metal);
  k.box(-hw + 0.05, hw - 0.05, 0, 0.03, top, top + PEGBOARD_RISE, s.panel);
}

// --- The core wall (design change: an 80s supercomputer cabinet bank) ---

/**
 * Jordi's design change (replacing the original quilted-panel wall): the
 * padded computer core reads as an homage to the big 80s war-game
 * supercomputer instead - a long, tall, dark charcoal cabinet bank. No
 * name, logo or text of any kind sits on it; the read is shape and light
 * alone.
 */

/** The cabinet's dark charcoal shell. */
const CABINET_CHARCOAL: Rgb = [0.1, 0.1, 0.11];

/** The narrow plinth and the thin top cap: darker than the shell. */
const CABINET_DARK: Rgb = [0.045, 0.045, 0.05];

/** A row's indicator colour: mostly red and amber, one white in five. */
const INDICATOR_TINTS: readonly Rgb[] = [
  STATUS_RED,
  STATUS_AMBER,
  STATUS_RED,
  STATUS_AMBER,
  [0.92, 0.94, 0.9],
];

/** How many vertical bays the cabinet's front is divided into. */
const BAY_COUNT = 6;
/** The gap between neighbouring bays, showing the dark divider between them. */
const BAY_GAP = 0.04;
/** The plinth's height and the cap's height, in metres. */
const CABINET_PLINTH_H = 0.15;
const CABINET_CAP_H = 0.1;
/** How many rows of indicator lights each bay carries. */
const BAY_ROWS = 10;
/** A row strip's height and the gap between rows. */
const ROW_H = 0.13;
const ROW_GAP = 0.05;
/** The bay's grid margin above the plinth, and its inset from the divider either side. */
const BAY_MARGIN = 0.05;
const BAY_INSET = 0.04;

/**
 * The computer's name, in Jordi's own words: a second design note (this
 * one an explicit, one-off exception to the homage rule, approved for
 * this single string only). Every identifier and test round it stays
 * generic; the string itself lives nowhere but here.
 */
const CORE_NAMEPLATE = "W.O.P.R.";

/** The nameplate's pixel size and its light-on-dark colour. */
const NAMEPLATE_PX = 0.013;
const NAMEPLATE_TINT: Rgb = [0.92, 0.94, 0.97];

/**
 * The padded computer core: a flush, two-edge dark charcoal cabinet bank.
 * A narrow darker plinth (h 0 to `CABINET_PLINTH_H`) and a thin cap (the
 * top `CABINET_CAP_H`) frame `BAY_COUNT` tall vertical bays, each a
 * recessed dark panel carrying `BAY_ROWS` rows of indicator lights,
 * merged into one glowing strip per row rather than a box per light (so
 * the whole wall stays cheap: `stays under the triangle budget` in
 * `heroModels.test.ts`, well under 1500). Each row's colour is mostly red
 * or amber, one in five white (`INDICATOR_TINTS`); its blink group is
 * `(bay + row) % 8`, so the twinkle bank's eight groups all churn
 * somewhere across the cabinet. Two round reel-like details sit between
 * bays for the era's read. `CORE_NAMEPLATE` sits centred on the top cap,
 * light block-pixel letters (`pixelPanel`) on the cap's own dark plate,
 * steady (`s.signal`, never blinking).
 */
const coreWall: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw, top } = heroHalf(kind, variant);
  k.box(-hw, hw, 0, 0.06, 0, CABINET_PLINTH_H, s.tinted(CABINET_DARK));
  k.box(-hw, hw, 0, 0.06, top - CABINET_CAP_H, top, s.tinted(CABINET_DARK));
  k.box(
    -hw,
    hw,
    0,
    0.05,
    CABINET_PLINTH_H,
    top - CABINET_CAP_H,
    s.tinted(CABINET_CHARCOAL),
  );

  const usableW = 2 * hw - (BAY_COUNT - 1) * BAY_GAP;
  const bayW = usableW / BAY_COUNT;
  const rowsH0 = CABINET_PLINTH_H + BAY_MARGIN;
  const rowsTop = rowsH0 + BAY_ROWS * (ROW_H + ROW_GAP);
  for (let bay = 0; bay < BAY_COUNT; bay++) {
    const a0 = -hw + bay * (bayW + BAY_GAP);
    const a1 = a0 + bayW;
    k.box(
      a0 + BAY_INSET,
      a1 - BAY_INSET,
      0.05,
      0.07,
      rowsH0 - BAY_MARGIN / 2,
      rowsTop,
      s.dark,
    );
    for (let row = 0; row < BAY_ROWS; row++) {
      const h0 = rowsH0 + row * (ROW_H + ROW_GAP);
      const tint = INDICATOR_TINTS[row % INDICATOR_TINTS.length] ?? STATUS_RED;
      const group = (bay + row) % 8;
      k.panel(
        a0 + BAY_INSET + 0.02,
        a1 - BAY_INSET - 0.02,
        0.07 + DECAL_LIFT,
        h0,
        h0 + ROW_H,
        s.blink(tint, group),
      );
    }
  }
  for (const gapAfterBay of [1, 4]) {
    const a = -hw + gapAfterBay * (bayW + BAY_GAP) - BAY_GAP / 2;
    const h = top - CABINET_CAP_H - 0.3;
    k.ring(a, 0.03, h, 0.11, 0.015, 4, 8, s.metal, "inward");
    k.ring(a, 0.03, h, 0.06, 0.01, 4, 8, s.dark, "inward");
  }

  const nameRows = textRows(CORE_NAMEPLATE);
  const nameCols = nameRows[0]?.length ?? 0;
  pixelPanel(
    k,
    nameRows,
    (-nameCols * NAMEPLATE_PX) / 2,
    top - 0.015,
    NAMEPLATE_PX,
    0.06 + DECAL_LIFT,
    (ch) => (ch === "#" ? s.signal(NAMEPLATE_TINT) : null),
  );
};

// --- The gun rack ----------------------------------------------------

/** The gun rack's backboard: its height band. */
const RACK_BACK_H0 = 0.7;
const RACK_BACK_H1 = 1.9;

/** The gun's base height on the rack's cradles. */
const RACK_GUN_H0 = 1.15;

/** The gun's box depth on the rack (kept under the flush 0.3 m limit). */
const RACK_GUN_D0 = 0.06;
const RACK_GUN_D1 = 0.28;

/**
 * The gun rack: flush on its wall. A dark backboard (h `RACK_BACK_H0` to
 * `RACK_BACK_H1`, the width less 0.05 m each side) carries two padded
 * cradle brackets at `a` +-0.55, and `bigGun` lies across them along `a`
 * from -0.8 to 0.85. The gun's core blinks group 0 of the breathe bank.
 */
const gunRack: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const bw = hw - 0.05;
  k.bevelBox(-bw, bw, 0, 0.04, RACK_BACK_H0, RACK_BACK_H1, 0.01, s.dark);
  for (const a of [-0.55, 0.55])
    k.bevelBox(
      a - 0.06,
      a + 0.06,
      0.04,
      RACK_GUN_D1 - 0.02,
      RACK_GUN_H0 - 0.14,
      RACK_GUN_H0 + 0.02,
      0.015,
      s.tinted(PAD_TINT),
    );
  bigGun(k, s, -0.8, 0.85, RACK_GUN_D0, RACK_GUN_D1, RACK_GUN_H0);
};

// --- The gun bench -----------------------------------------------------

/** The bigGun's placement zone on the gun bench, and its depth and height. */
const BENCH_GUN_A0 = -0.5;
const BENCH_GUN_A1 = 0.9;
const BENCH_GUN_D0 = 0.32;
const BENCH_GUN_D1 = 0.54;
const BENCH_GUN_H0 = 0.96;

/** Where the pegboard's tool silhouettes hang: their a-centres and heights. */
const BENCH_TOOLS: readonly { a: number; h0: number; h1: number }[] = [
  { a: -0.6, h0: 1.0, h1: 1.32 },
  { a: -0.25, h0: 1.05, h1: 1.24 },
  { a: 0.55, h0: 0.98, h1: 1.3 },
];

/**
 * The gun bench: `workbench` at `WORKBENCH_TOP`, `bigGun` resting on two
 * padded blocks over its right part (`a` -0.5 to 0.9), and a few tool
 * silhouettes hanging on the pegboard. The bench's left end (`a` -0.9 to
 * -0.55) is left clear: the catalogue's surface. The gun's core blinks
 * group 0 of the breathe bank.
 */
const gunBench: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw, d1 } = heroHalf(kind, variant);
  workbench(k, s, hw, d1, WORKBENCH_TOP);
  for (const a of [-0.3, 0.65])
    k.bevelBox(
      a - 0.08,
      a + 0.08,
      BENCH_GUN_D0 - 0.02,
      BENCH_GUN_D1 + 0.02,
      WORKBENCH_TOP,
      BENCH_GUN_H0,
      0.01,
      s.tinted(PAD_TINT),
    );
  bigGun(
    k,
    s,
    BENCH_GUN_A0,
    BENCH_GUN_A1,
    BENCH_GUN_D0,
    BENCH_GUN_D1,
    BENCH_GUN_H0,
  );
  for (const t of BENCH_TOOLS)
    k.box(t.a - 0.015, t.a + 0.015, 0.03, 0.05, t.h0, t.h1, s.dark);
};

// --- The tube bench ----------------------------------------------------

/** The open metal box holding the tube hub, in the recipe's local terms. */
const BOX_A0 = 0.1;
const BOX_A1 = 0.7;
const BOX_D0 = 0.3;
const BOX_D1 = 0.7;
const BOX_H0 = WORKBENCH_TOP;
const BOX_H1 = 1.4;

/** The Y of tubes: the hub's centre in (a, h) and the tube arms' reach. */
const HUB_A = 0.4;
const HUB_H = 1.12;
const HUB_D = 0.615;
const HUB_RADIUS = 0.06;
const TUBE_LEN = 0.24;
const TUBE_WIDTH = 0.03;
const TUBE_D0 = 0.6;
const TUBE_D1 = 0.63;

/** The Y's three arm directions, 120 degrees apart, in radians. */
const TUBE_ANGLES: readonly number[] = [
  Math.PI / 2,
  (7 * Math.PI) / 6,
  (11 * Math.PI) / 6,
];

/**
 * The Y of three glowing tubes: a small dark hub cylinder at
 * `(HUB_A, HUB_D, HUB_H)` and three thin `tiltedBar` extrusions in the
 * box's front plane (`extrude`, at fixed depth: the outline is in the
 * `(a, h)` plane), one per `TUBE_ANGLES` direction, each spanning exactly
 * from the hub's centre to its tip so every tube reaches the hub within
 * the model checks' contact distance.
 */
function tubeY(k: Kit, s: Surfaces): void {
  k.cylinder(
    HUB_A,
    HUB_D,
    HUB_H - HUB_RADIUS * 0.6,
    HUB_H + HUB_RADIUS * 0.6,
    HUB_RADIUS,
    8,
    s.dark,
  );
  TUBE_ANGLES.forEach((angle, i) => {
    const tipA = HUB_A + TUBE_LEN * Math.cos(angle);
    const tipH = HUB_H + TUBE_LEN * Math.sin(angle);
    const bar = tiltedBar(
      (HUB_A + tipA) / 2,
      (HUB_H + tipH) / 2,
      angle,
      TUBE_LEN,
      TUBE_WIDTH,
    );
    const tint = TUBE_TINTS[i] ?? CORE_GREEN;
    k.extrude(bar, TUBE_D0, TUBE_D1, s.blink(tint, i));
  });
}

/** The row of five side lights on the box's right wall: their depths. */
const SIDE_LIGHT_D: readonly number[] = [0.35, 0.43, 0.51, 0.59, 0.67];

/** The hinged panel's hinge point and its lean, in the (d, h) plane. */
const PANEL_A_MID = 0.85;
const PANEL_HINGE_D = 0.55;
const PANEL_HINGE_H = WORKBENCH_TOP;
const PANEL_LEN = 0.35;
const PANEL_ANGLE = (70 * Math.PI) / 180;

/** The soldering lamp's base, its two arm joints, in (d, h). */
const LAMP_A = -0.1;
const LAMP_BASE_D = 0.25;
const LAMP_BASE_TOP = WORKBENCH_TOP + 0.04;
const LAMP_ARM1_ANGLE = (80 * Math.PI) / 180;
const LAMP_ARM1_LEN = 0.5;
const LAMP_ARM2_ANGLE = (55 * Math.PI) / 180;
const LAMP_ARM2_LEN = 0.42;

/**
 * The tube bench: `workbench` at `WORKBENCH_TOP`. On its right part an
 * open metal box (`BOX_A0..BOX_A1`, `BOX_D0..BOX_D1`, `BOX_H0..BOX_H1`,
 * back, sides and top, open front) holds the tube Y (`tubeY`) and a row
 * of five side lights on its right wall (groups 3 to 7 of the chase
 * bank). Beside it a hinged panel leans open (`tiltedBar` in
 * `profileAlong`), two tools, a coiled cable run and a soldering lamp on a
 * weighted base, its two jointed arm bars rising to about 1.9 m and its
 * cone-shade head's bulb glowing down (`s.signal`). The bench's left part
 * (`a` -0.9 to -0.3) is left clear: the catalogue's surface.
 */
const tubeBench: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, d1 } = heroHalf(kind, variant);
  const f = frameAt([0, 0, 0], 0);
  workbench(k, s, hw, d1, WORKBENCH_TOP);

  k.box(BOX_A0, BOX_A1, BOX_D0, BOX_D0 + 0.02, BOX_H0, BOX_H1, s.metal);
  k.box(BOX_A0, BOX_A0 + 0.02, BOX_D0, BOX_D1, BOX_H0, BOX_H1, s.metal);
  k.box(BOX_A1 - 0.02, BOX_A1, BOX_D0, BOX_D1, BOX_H0, BOX_H1, s.metal);
  k.box(BOX_A0, BOX_A1, BOX_D0, BOX_D1, BOX_H1 - 0.02, BOX_H1, s.metal);
  tubeY(k, s);
  SIDE_LIGHT_D.forEach((d, i) => {
    const tint = STATUS_TINTS[i % STATUS_TINTS.length] ?? STATUS_GREEN;
    k.box(
      BOX_A1 - 0.02,
      BOX_A1 + 0.015,
      d - 0.012,
      d + 0.012,
      1.13,
      1.16,
      s.blink(tint, 3 + i),
    );
  });

  const panelTipD = PANEL_HINGE_D + PANEL_LEN * Math.cos(PANEL_ANGLE);
  const panelTipH = PANEL_HINGE_H + PANEL_LEN * Math.sin(PANEL_ANGLE);
  profileAlong(
    kitAt,
    f,
    tiltedBar(
      (PANEL_HINGE_D + panelTipD) / 2,
      (PANEL_HINGE_H + panelTipH) / 2,
      PANEL_ANGLE,
      PANEL_LEN,
      0.02,
    ),
    PANEL_A_MID - 0.05,
    PANEL_A_MID + 0.05,
    s.metal,
  );

  k.box(-0.2, -0.04, 0.15, 0.4, WORKBENCH_TOP, WORKBENCH_TOP + 0.03, s.dark);
  k.box(0.0, 0.08, 0.45, 0.62, WORKBENCH_TOP, WORKBENCH_TOP + 0.025, s.metal);

  k.cylinderAlong(-0.25, -0.1, 0.78, 0.03, 0.012, 6, s.dark);
  for (let i = 0; i < 3; i++)
    k.ring(-0.2, 0.78, 0.02 + i * 0.02, 0.04, 0.009, 4, 10, s.dark, "up");

  k.bevelBox(
    LAMP_A - 0.07,
    LAMP_A + 0.01,
    LAMP_BASE_D - 0.1,
    LAMP_BASE_D + 0.1,
    WORKBENCH_TOP,
    LAMP_BASE_TOP,
    0.01,
    s.dark,
  );
  const joint1D = LAMP_BASE_D + LAMP_ARM1_LEN * Math.cos(LAMP_ARM1_ANGLE);
  const joint1H = LAMP_BASE_TOP + LAMP_ARM1_LEN * Math.sin(LAMP_ARM1_ANGLE);
  profileAlong(
    kitAt,
    f,
    tiltedBar(
      (LAMP_BASE_D + joint1D) / 2,
      (LAMP_BASE_TOP + joint1H) / 2,
      LAMP_ARM1_ANGLE,
      LAMP_ARM1_LEN,
      0.03,
    ),
    LAMP_A - 0.015,
    LAMP_A + 0.015,
    s.metal,
  );
  const joint2D = joint1D + LAMP_ARM2_LEN * Math.cos(LAMP_ARM2_ANGLE);
  const joint2H = joint1H + LAMP_ARM2_LEN * Math.sin(LAMP_ARM2_ANGLE);
  profileAlong(
    kitAt,
    f,
    tiltedBar(
      (joint1D + joint2D) / 2,
      (joint1H + joint2H) / 2,
      LAMP_ARM2_ANGLE,
      LAMP_ARM2_LEN,
      0.025,
    ),
    LAMP_A - 0.0125,
    LAMP_A + 0.0125,
    s.metal,
  );
  k.lathe(
    LAMP_A,
    joint2D,
    [
      [0, joint2H],
      [0.11, joint2H + 0.06],
      [0.09, joint2H + 0.11],
      [0, joint2H + 0.13],
    ],
    10,
    s.dark,
  );
  k.cylinder(
    LAMP_A,
    joint2D,
    joint2H - 0.02,
    joint2H,
    0.06,
    10,
    s.signal(LAMP_WARM),
  );
};

// --- The field pack ----------------------------------------------------

/** The pack body's fixed size, whatever the variant or the stand. */
const PACK_W = 0.45;
const PACK_D = 0.3;
const PACK_H = 0.7;

/** How far the pack's top leans back of its bottom, in metres (v0 only). */
const PACK_LEAN = 0.08;

/** v0's body sits this high off the floor, on its A-frame stand. */
const V0_BOTTOM = 0.18;

/** v1's body sits with its bottom here, on its rack (H8's catalogue top). */
const V1_BOTTOM = 0.9;

/** Where along the body's height the status bar and the strap sit, 0 to 1. */
const BAR_FRACTION = 0.55;

/** The wand's height and the coil's turns. */
const WAND_TOP = 1.6;
const COIL_TURNS = 4;

/** The two ribbed cylinders' radius and where they stand off the body's side. */
const RIB_RADIUS = 0.07;
const RIB_A = PACK_W / 2 + RIB_RADIUS + 0.02;
const RIB_D0 = -0.09;
const RIB_D1 = 0.09;

/**
 * The pack body's front depth at height `h` (metres from the floor):
 * `PACK_D / 2` for v1's upright box, sloping back by `PACK_LEAN` from
 * bottom to top for v0's leaning body, so a part built against the front
 * face touches it exactly at the height it is built.
 */
function packFrontAt(h: number, variant: number): number {
  if (variant !== 0) return PACK_D / 2;
  const frac = Math.min(1, Math.max(0, (h - V0_BOTTOM) / PACK_H));
  return PACK_D / 2 - PACK_LEAN * frac;
}

/**
 * A ribbed cylinder: an upright metal tube with three thin rings standing
 * proud of it, centred at `(a, d)` from `h0` to `h1`.
 */
function ribbedCylinder(
  k: Kit,
  s: Surfaces,
  a: number,
  d: number,
  h0: number,
  h1: number,
): void {
  k.cylinder(a, d, h0, h1, RIB_RADIUS, 10, s.metal);
  for (let i = 1; i <= 3; i++) {
    const h = h0 + ((h1 - h0) * i) / 4;
    k.ring(a, d, h, RIB_RADIUS + 0.012, 0.01, 4, 12, s.dark, "up");
  }
}

/**
 * The two ribbed cylinders on the pack's side and the hose loop
 * connecting them: two short `cylinderAlong` pieces, drawn in a frame
 * yawed a quarter turn so their length runs along `d` (across the two
 * cylinders) instead of `a`.
 */
function ribsAndHose(
  k: Kit,
  kitAt: KitAt,
  s: Surfaces,
  f: Frame,
  h0: number,
  h1: number,
): void {
  ribbedCylinder(k, s, RIB_A, RIB_D0, h0, h1);
  ribbedCylinder(k, s, RIB_A, RIB_D1, h0, h1);
  const cross = kitAt(yawed(f, RIB_A, 0, Math.PI / 2));
  for (const h of [h0 + 0.06, h1 - 0.06])
    cross.cylinderAlong(RIB_D0, RIB_D1, 0, h, 0.02, 8, s.dark);
}

/**
 * A wand on a coiled cable: a long thin cylinder with a small grip band,
 * standing to `WAND_TOP` at `(a, d)`, its tip glowing group 5 of the
 * status bank; the coil is `COIL_TURNS` stacked rings climbing from the
 * floor beside its base.
 */
function wandAndCoil(
  k: Kit,
  s: Surfaces,
  a: number,
  d: number,
  base: number,
): void {
  k.cylinder(a, d, base, WAND_TOP - 0.03, 0.014, 8, s.metal);
  k.cylinder(a, d, base + 0.08, base + 0.16, 0.022, 8, s.dark);
  k.cylinder(a, d, WAND_TOP - 0.03, WAND_TOP, 0.02, 8, s.blink(WAND_CYAN, 5));
  for (let i = 0; i < COIL_TURNS; i++)
    k.ring(
      a + 0.05,
      d,
      base + 0.02 + i * 0.025,
      0.035,
      0.007,
      4,
      10,
      s.dark,
      "up",
    );
}

/**
 * v0's body: `profileAlong` of a parallelogram, its top `PACK_LEAN` behind
 * its bottom, resting on two splayed `tiltedBar` legs of a small A-frame
 * (as `turret`'s legs, a foot pad each), its base on the floor.
 */
function packV0(k: Kit, kitAt: KitAt, f: Frame, s: Surfaces): void {
  const dBack = -PACK_D / 2;
  const dFront = PACK_D / 2;
  profileAlong(
    kitAt,
    f,
    [
      [dBack, V0_BOTTOM],
      [dFront, V0_BOTTOM],
      [dFront - PACK_LEAN, V0_BOTTOM + PACK_H],
      [dBack - PACK_LEAN, V0_BOTTOM + PACK_H],
    ],
    -PACK_W / 2,
    PACK_W / 2,
    s.tinted(PACK_GREY),
  );
  // The floor end sits a little above 0: a tilted bar's thickness runs
  // perpendicular to its length, so a corner at the floor end would dip
  // below the true floor if the bar's own centreline reached all the way
  // down to h 0 (the lesson behind the turret's legs stopping at 0.03).
  const footH = 0.03;
  const apex = { d: -0.02, h: V0_BOTTOM + 0.05 };
  for (const footD of [dBack - 0.05, dFront + 0.05]) {
    const angle = Math.atan2(footH - apex.h, footD - apex.d);
    const length = Math.hypot(footD - apex.d, footH - apex.h);
    const bar = tiltedBar(
      (apex.d + footD) / 2,
      (apex.h + footH) / 2,
      angle,
      length,
      0.03,
    );
    profileAlong(kitAt, f, bar, -0.03, 0.03, s.dark);
    k.cylinder(0, footD, 0, footH, 0.045, 8, s.dark);
  }
}

/**
 * v1's body: an upright bevelled box, its bottom at `V1_BOTTOM`, standing
 * on a rack of four corner posts and two rails.
 */
function packV1(k: Kit, s: Surfaces): void {
  k.bevelBox(
    -PACK_W / 2,
    PACK_W / 2,
    -PACK_D / 2,
    PACK_D / 2,
    V1_BOTTOM,
    V1_BOTTOM + PACK_H,
    HOUSING_BEVEL,
    s.tinted(PACK_GREY),
  );
  for (const a of [-PACK_W / 2 - 0.06, PACK_W / 2 + 0.06])
    for (const d of [-PACK_D / 2 - 0.02, PACK_D / 2 + 0.02])
      k.box(
        a - 0.02,
        a + 0.02,
        d - 0.02,
        d + 0.02,
        0,
        V1_BOTTOM + 0.02,
        s.metal,
      );
  for (const h of [V1_BOTTOM - 0.05, 0.35])
    k.box(
      -PACK_W / 2 - 0.08,
      PACK_W / 2 + 0.08,
      -PACK_D / 2 - 0.04,
      -PACK_D / 2,
      h,
      h + 0.03,
      s.metal,
    );
}

/**
 * The field pack: a bulky grey backpack unit on a metal frame (`packV0`
 * leaning on an A-frame stand, `packV1` upright on a rack), two ribbed
 * cylinders and a hose loop on its side (`ribsAndHose`), a small bar of
 * five status lights on its front (groups 0 to 4 of the status bank, each
 * built to touch the body's own front face, straight or leaning) and a
 * wand on a coiled cable beside it, its tip blinking group 5.
 */
const fieldPack: HeroRecipe = ({ k, kitAt, s, variant }) => {
  const f = frameAt([0, 0, 0], 0);
  const bottom = variant === 0 ? V0_BOTTOM : V1_BOTTOM;
  const barH = bottom + PACK_H * BAR_FRACTION;
  const front = packFrontAt(barH, variant);
  STATUS_TINTS.forEach((tint, i) => {
    const a = -0.15 + i * 0.08;
    k.box(
      a - 0.018,
      a + 0.018,
      front - 0.002,
      front + 0.018,
      barH,
      barH + 0.03,
      s.blink(tint, i),
    );
  });

  if (variant === 0) {
    packV0(k, kitAt, f, s);
    ribsAndHose(k, kitAt, s, f, bottom + 0.08, bottom + 0.48);
    wandAndCoil(k, s, -PACK_W / 2 - 0.15, 0, 0);
  } else {
    packV1(k, s);
    ribsAndHose(k, kitAt, s, f, bottom + 0.08, bottom + 0.48);
    wandAndCoil(k, s, PACK_W / 2 + 0.12, 0, bottom);
  }
};

/** The workshop kinds' recipes. */
export const WORKSHOP_RECIPES = {
  "core-wall": coreWall,
  "gun-rack": gunRack,
  "gun-bench": gunBench,
  "tube-bench": tubeBench,
  "field-pack": fieldPack,
} satisfies Record<
  Extract<
    HeroKind,
    "core-wall" | "gun-rack" | "gun-bench" | "tube-bench" | "field-pack"
  >,
  HeroRecipe
>;
