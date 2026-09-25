/**
 * The workshop heroes' recipes: the core wall, the gun rack, the gun
 * bench, the tube bench and the field pack. What they share is the
 * workbench and the wall rack: heavy benches with tools and cables (the
 * two benches), wall frames that hold a piece (the gun rack, the cabinet
 * bank of the core wall), ribbed hoses and tubes (the tube bench, the
 * pack) and glowing cores, tubes and status lights.
 *
 * Two helpers carry the shared shapes: `bigGun` (the oversized energy
 * weapon, identical wherever it is called, only its placement moves, with
 * `gunRests` saying where its supports go so they never meet its grip) and
 * `workbench` (the heavy desk under the gun bench and the tube bench: a
 * top slab, four legs, a lower shelf reaching them and a blank pegboard
 * rising `PEGBOARD_RISE` above the slab). The benches' top height and
 * their clear ends come from the catalogue's surface (`benchSurface`), so
 * no recipe repeats a catalogue number.
 *
 * A leaning or tilted part (a strut, a jointed lamp arm, the leaning pack)
 * is built with `tiltedBar` or a slanted profile inside `profileAlong`, in
 * the frame's own `(d, h)` plane; a part that swings about the vertical
 * (the tube box's open side door) is a plain box in a `yawed` frame; a
 * part that tilts in the wall plane (a tube of the Y, a gun grip) is a
 * `tiltedBar` given straight to `k.extrude`.
 */

import { HERO_CATALOGUE, type HeroSurfaceSpec } from "../../../world/heroes";
import type { HeroKind } from "../../../world/types";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import {
  discOutline,
  profileAlong,
  SPARK_TINT,
  tiltedBar,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import {
  cornerPosts,
  heroHalf,
  STATUS_AMBER,
  STATUS_GREEN,
  type HeroRecipe,
} from "./common";
import { pixelPanel, textRows } from "./pixels";

/** A status light that says trouble, and a tool's red handle. */
const STATUS_RED: Rgb = [1.0, 0.15, 0.1];

/** A row of status lights' colours, groups in order. */
const STATUS_TINTS: readonly Rgb[] = [
  STATUS_GREEN,
  STATUS_GREEN,
  STATUS_AMBER,
  STATUS_GREEN,
  STATUS_RED,
];

/** The chamfer of a hero's housings, in metres. */
const HOUSING_BEVEL = 0.02;

/** A frame at the origin, facing the way every recipe here is built. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

// --- The big gun ---------------------------------------------------

/** The big gun's shell: a dark gunmetal grey, darker than any look's metal. */
const GUN_GREY: Rgb = [0.2, 0.2, 0.22];

/** The big gun's core window and muzzle glow: a vivid plasma green. */
const CORE_GREEN: Rgb = [0.25, 0.95, 0.3];

/** The rubber pads a gun rests on, on the rack and the bench: a worn brown-grey. */
const PAD_RUBBER: Rgb = [0.35, 0.31, 0.27];

/** The big gun's overall length, stock to muzzle, in metres. */
const GUN_LENGTH = 1.3;
/** The body's depth (back to front) and height, in metres. */
const GUN_DEPTH = 0.2;
const GUN_HEIGHT = 0.3;
/** The stock's length at the rear, its height band above `h0` and its depth. */
const STOCK_LEN = 0.2;
const STOCK_H0 = 0.05;
const STOCK_H1 = 0.25;
const STOCK_DEPTH = 0.14;
/** The body's length after the stock; the square barrel takes the rest. */
const BODY_LEN = 0.6;
/** The square barrel's side and its vent collars' side, in metres. */
const BARREL_SIDE = 0.16;
const COLLAR_SIDE = 0.19;
/** The core window on the body's front: its span along the body and its margins. */
const WINDOW_A0 = 0.1;
const WINDOW_A1 = 0.52;
const WINDOW_H0 = 0.06;
const WINDOW_H1 = 0.24;
/** How far the window's dark bezel stands proud of the body's front. */
const BEZEL_T = 0.012;
/** The grip's top point along the body (from the body's rear), and how far it hangs. */
const GRIP_AT = 0.12;
/** How far the grip hangs below the gun's belly: every support keeps this clear. */
const GRIP_DROP = 0.14;
/** Where the barrel's rest sits along it, between its first two vent collars. */
const BARREL_REST = 0.165;
/** The vent collars' places along the barrel. */
const COLLARS: readonly number[] = [0.08, 0.25, 0.42];
/** A gun rest's half width along `a`. */
const REST_HALF = 0.06;

/**
 * Where the big gun's three rests go for a gun placed with `bigGun(k, s,
 * a0, a1, ..., h0)`: under the stock, under the body's front end and under
 * the barrel, each with the height its top must reach to carry the gun
 * there. Every rest is `REST_HALF` either side of its `a` and stays clear
 * of the grip, which hangs between the stock rest and the body rest.
 */
export function gunRests(
  a0: number,
  a1: number,
  h0: number,
): { a: number; top: number }[] {
  const aStart = (a0 + a1) / 2 - GUN_LENGTH / 2;
  return [
    { a: aStart + STOCK_LEN / 2, top: h0 + STOCK_H0 },
    { a: aStart + STOCK_LEN + BODY_LEN - 0.08, top: h0 },
    {
      a: aStart + STOCK_LEN + BODY_LEN + BARREL_REST,
      top: h0 + (GUN_HEIGHT - BARREL_SIDE) / 2,
    },
  ];
}

/**
 * The big green gun: an oversized energy weapon lying along `a`, the same
 * part sizes wherever it is called (only its placement moves). It is
 * `GUN_LENGTH` long, centred between `a0` and `a1`, and its body
 * `GUN_DEPTH` deep, centred between `d0` and `d1` (the caller keeps both
 * spans at least that long; nothing is stretched to fill them), and
 * `GUN_HEIGHT` tall from `h0`. From
 * the rear: a narrower stock, a bulky bevelled body with a large glowing
 * green core window on its front face in a dark bezel and a dark spine rib
 * on top, and a long square barrel with three vent collars, dark vent
 * slots between them and a green glow in its muzzle. A raked dark grip
 * hangs `GRIP_DROP` under the body's rear; `gunRests` says where supports
 * go to miss it. The window and the muzzle are blink group 0 (the kind's
 * breathe bank).
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
  const aStart = (a0 + a1) / 2 - GUN_LENGTH / 2;
  const aBody = aStart + STOCK_LEN;
  const aBarrel = aBody + BODY_LEN;
  const aEnd = aStart + GUN_LENGTH;
  const dBack = (d0 + d1) / 2 - GUN_DEPTH / 2;
  const dFront = dBack + GUN_DEPTH;
  const dc = (dBack + dFront) / 2;
  const hTop = h0 + GUN_HEIGHT;
  const hc = h0 + GUN_HEIGHT / 2;
  const shell = s.tinted(GUN_GREY);
  const core = s.blink(CORE_GREEN, 0);

  k.bevelBox(
    aStart,
    aBody + 0.02,
    dc - STOCK_DEPTH / 2,
    dc + STOCK_DEPTH / 2,
    h0 + STOCK_H0,
    h0 + STOCK_H1,
    HOUSING_BEVEL,
    shell,
  );
  k.bevelBox(aBody, aBarrel, dBack, dFront, h0, hTop, 0.025, shell);
  k.box(
    aBody + 0.1,
    aBarrel - 0.08,
    dc - 0.04,
    dc + 0.04,
    hTop,
    hTop + 0.025,
    s.dark,
  );
  k.box(
    aBody + WINDOW_A0 - 0.025,
    aBody + WINDOW_A1 + 0.025,
    dFront,
    dFront + BEZEL_T,
    h0 + WINDOW_H0 - 0.025,
    h0 + WINDOW_H1 + 0.025,
    s.dark,
  );
  k.panel(
    aBody + WINDOW_A0,
    aBody + WINDOW_A1,
    dFront + BEZEL_T + DECAL_LIFT,
    h0 + WINDOW_H0,
    h0 + WINDOW_H1,
    core,
  );

  const b = BARREL_SIDE / 2;
  const c = COLLAR_SIDE / 2;
  k.box(aBarrel, aEnd - 0.005, dc - b, dc + b, hc - b, hc + b, shell);
  const collars = COLLARS.map((t) => aBarrel + t);
  for (const a of collars)
    k.box(a - 0.02, a + 0.02, dc - c, dc + c, hc - c, hc + c, s.dark);
  for (let i = 0; i + 1 < collars.length; i++) {
    const [ca, cb] = [collars[i] ?? 0, collars[i + 1] ?? 0];
    for (const t of [0.35, 0.65]) {
      const a = ca + (cb - ca) * t;
      k.panel(
        a - 0.008,
        a + 0.008,
        dc + b + DECAL_LIFT,
        hc - 0.05,
        hc + 0.05,
        s.dark,
      );
    }
  }
  k.box(aEnd - 0.03, aEnd, dc - 0.05, dc + 0.05, hc - 0.05, hc + 0.05, core);

  const gripTopA = aBody + GRIP_AT;
  const gripFootA = gripTopA - 0.06;
  const gripFootH = h0 - GRIP_DROP + 0.015;
  const gripTopH = h0 + 0.02;
  k.extrude(
    tiltedBar(
      (gripTopA + gripFootA) / 2,
      (gripTopH + gripFootH) / 2,
      Math.atan2(gripTopH - gripFootH, gripTopA - gripFootA),
      Math.hypot(gripTopA - gripFootA, gripTopH - gripFootH),
      0.05,
    ),
    dc - 0.03,
    dc + 0.03,
    s.dark,
  );
}

// --- The workbench ---------------------------------------------------

/** How far above the top slab the pegboard rises, in metres. */
const PEGBOARD_RISE = 0.5;

/** The thickness of the workbench's top slab. */
const SLAB_THICK = 0.05;

/** How far the legs' centres stand in from the slab's ends and from its back and front. */
const LEG_IN_A = 0.08;
const LEG_IN_D = 0.09;
/** Half a leg's side. */
const LEG_HALF = 0.03;

/** The lower shelf's height band. */
const SHELF_H0 = 0.26;
const SHELF_H1 = 0.31;

/**
 * A bench kind's one catalogue surface: its top height and its clear end.
 * Throws for a kind without one (only the benches call it).
 */
function benchSurface(kind: HeroKind): HeroSurfaceSpec {
  const surface = HERO_CATALOGUE[kind].surfaces[0];
  if (surface === undefined)
    throw new Error(`workshop: ${kind} has no bench surface`);
  return surface;
}

/**
 * A heavy workbench, backed against its wall: a top slab from
 * `top - SLAB_THICK` to `top` over the whole footprint, four square legs
 * (`cornerPosts`) standing `LEG_IN_A` in from its ends and `LEG_IN_D` in
 * from its back and front, a lower shelf spanning leg centre to leg centre
 * so it rests in all four, and a blank pegboard panel standing at the wall
 * from `top` up to `top + PEGBOARD_RISE`. `hw` is the bench's half width
 * and `depth` its footprint depth (`heroHalf`'s `d1`); every recipe that
 * calls it adds its own gear on the slab and the board.
 */
export function workbench(
  k: Kit,
  s: Surfaces,
  hw: number,
  depth: number,
  top: number,
): void {
  k.bevelBox(-hw, hw, 0, depth, top - SLAB_THICK, top, 0.015, s.body);
  const legA = hw - LEG_IN_A;
  const legD = [LEG_IN_D, depth - LEG_IN_D];
  cornerPosts(k, [-legA, legA], legD, LEG_HALF, 0, top - SLAB_THICK, s.metal);
  k.box(-legA, legA, LEG_IN_D, depth - LEG_IN_D, SHELF_H0, SHELF_H1, s.metal);
  k.box(-hw + 0.05, hw - 0.05, 0, 0.03, top, top + PEGBOARD_RISE, s.panel);
}

// --- The core wall -------------------------------------------------------

/** The cabinets' matte black. */
const CABINET_BLACK: Rgb = [0.08, 0.08, 0.09];

/** The shadowed back panel showing in the gaps between cabinets. */
const GAP_BLACK: Rgb = [0.03, 0.03, 0.035];

/** The lamp plates, the control ledge and the nameplate: a dark bezel grey. */
const BEZEL_GREY: Rgb = [0.25, 0.25, 0.27];

/** A red indicator lamp. */
const LAMP_RED: Rgb = [0.85, 0.1, 0.1];

/** An amber indicator lamp, warm like an incandescent bulb. */
const LAMP_AMBER: Rgb = [0.95, 0.55, 0.1];

/** A white indicator lamp and the nameplate's letters: a warm bulb white. */
const LAMP_WHITE: Rgb = [0.95, 0.93, 0.85];

/** The lamps' colours, picked by hash: mostly red and amber, one in eight white. */
const LAMP_TINTS: readonly Rgb[] = [
  LAMP_RED,
  LAMP_AMBER,
  LAMP_RED,
  LAMP_AMBER,
  LAMP_RED,
  LAMP_AMBER,
  LAMP_RED,
  LAMP_WHITE,
];

/** How many cabinets the bank is divided into, and the gap between two. */
const BAY_COUNT = 6;
const BAY_GAP = 0.05;
/** The cabinets' back (in front of the back panel) and front faces, in `d`. */
const CAB_BACK = 0.04;
const CAB_FRONT = 0.22;
/** The plinth's top and the cornice's bottom: the cabinets stand between. */
const PLINTH_H = 0.12;
const CORNICE_H0 = 2.1;
/** How far the cornice reaches out from the wall. */
const CORNICE_D = 0.24;
/** A cabinet's lamp grid: columns, rows and its height band. */
const LAMP_COLS = 6;
const LAMP_ROWS = 13;
const LAMP_H0 = 1.1;
const LAMP_H1 = 2.02;
/** The side of one square lamp, in metres. */
const LAMP_SIDE = 0.045;
/** The lamp plate's margin round its grid, and how far it stands proud of the cabinet. */
const PLATE_MARGIN = 0.03;
const PLATE_T = 0.01;
/** The control ledge: its height at the cabinets and at its front edge (d 0.3). */
const LEDGE_BACK_H0 = 0.84;
const LEDGE_BACK_H1 = 0.96;
const LEDGE_FRONT_H0 = 0.86;
const LEDGE_FRONT_H1 = 0.89;
/** The flush limit the ledge's front edge stops at. */
const LEDGE_FRONT = 0.3;

/**
 * The name on the bank's top plate. The one approved exception to the
 * rule that no name from a film or game appears in this code: this string
 * only, nowhere else, and every identifier and test round it stays generic.
 */
const CORE_NAMEPLATE = "W.O.P.R.";

/** The nameplate's pixel size, in metres. */
const NAMEPLATE_PX = 0.017;
/** The raised nameplate's margin round its letters, and its thickness. */
const NAMEPLATE_MARGIN = 0.03;
const NAMEPLATE_T = 0.02;

/**
 * A lamp's hash: an integer mix of its cabinet, row and column, so
 * neighbouring lamps land in unrelated groups and tints and the grid
 * churns with no stripes. Its low three bits pick the blink group, the
 * next three the tint.
 */
function lampHash(bay: number, row: number, col: number): number {
  let h =
    Math.imul(bay + 1, 0x9e3779b1) ^
    Math.imul(row + 1, 0x85ebca6b) ^
    Math.imul(col + 1, 0xc2b2ae35);
  h ^= h >>> 15;
  h = Math.imul(h, 0x2c1b3c6d);
  h ^= h >>> 12;
  return h >>> 0;
}

/**
 * The core wall: a flush, two-edge bank of big computer cabinets. A back
 * panel in shadow black, a plinth and a cornice span the width; between
 * them stand `BAY_COUNT` separate matte black bevelled cabinets
 * (`CAB_BACK` to `CAB_FRONT` deep) with a dark gap between each two, so
 * the bank reads as cabinets side by side, not one flat panel. Each
 * cabinet's upper part carries a raised grey lamp plate with a dense grid
 * of `LAMP_COLS` by `LAMP_ROWS` small square lamps; a lamp's tint (mostly
 * red and amber, one in eight white) and its twinkle group both come from
 * `lampHash`, so all eight groups churn all over the bank. A slim sloped
 * control ledge runs in front at desk height with a switch per cabinet,
 * and `CORE_NAMEPLATE` stands in light block-pixel letters (`s.signal`,
 * steady) on a raised grey plate on the cornice.
 */
const coreWall: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, top } = heroHalf(kind, variant);
  const cabinet = s.tinted(CABINET_BLACK);
  const bezel = s.tinted(BEZEL_GREY);
  k.box(-hw, hw, 0, CAB_BACK, PLINTH_H, CORNICE_H0, s.tinted(GAP_BLACK));
  k.box(-hw, hw, 0, CAB_FRONT, 0, PLINTH_H, cabinet);
  k.box(-hw, hw, 0, CORNICE_D, CORNICE_H0, top, cabinet);

  const bayW = (2 * hw - (BAY_COUNT - 1) * BAY_GAP) / BAY_COUNT;
  const rowPitch = (LAMP_H1 - LAMP_H0) / LAMP_ROWS;
  const lampFront = CAB_FRONT + PLATE_T + DECAL_LIFT;
  for (let bay = 0; bay < BAY_COUNT; bay++) {
    const a0 = -hw + bay * (bayW + BAY_GAP);
    const a1 = a0 + bayW;
    k.bevelBox(
      a0,
      a1,
      CAB_BACK,
      CAB_FRONT,
      PLINTH_H,
      CORNICE_H0,
      0.015,
      cabinet,
    );
    const g0 = a0 + PLATE_MARGIN + 0.02;
    const g1 = a1 - PLATE_MARGIN - 0.02;
    k.box(
      g0 - PLATE_MARGIN,
      g1 + PLATE_MARGIN,
      CAB_FRONT,
      CAB_FRONT + PLATE_T,
      LAMP_H0 - PLATE_MARGIN,
      LAMP_H1 + PLATE_MARGIN - (rowPitch - LAMP_SIDE),
      bezel,
    );
    const colPitch = (g1 - g0 - LAMP_SIDE) / (LAMP_COLS - 1);
    for (let row = 0; row < LAMP_ROWS; row++)
      for (let col = 0; col < LAMP_COLS; col++) {
        const hash = lampHash(bay, row, col);
        const tint = LAMP_TINTS[(hash >>> 3) % LAMP_TINTS.length] ?? LAMP_RED;
        const la = g0 + col * colPitch;
        const lh = LAMP_H0 + row * rowPitch;
        k.panel(
          la,
          la + LAMP_SIDE,
          lampFront,
          lh,
          lh + LAMP_SIDE,
          s.blink(tint, hash % 8),
        );
      }
    const am = (a0 + a1) / 2;
    k.box(am - 0.015, am + 0.015, 0.25, 0.27, 0.9, 0.96, s.metal);
  }
  profileAlong(
    kitAt,
    ORIGIN,
    [
      [CAB_FRONT, LEDGE_BACK_H0],
      [LEDGE_FRONT, LEDGE_FRONT_H0],
      [LEDGE_FRONT, LEDGE_FRONT_H1],
      [CAB_FRONT, LEDGE_BACK_H1],
    ],
    -hw + 0.1,
    hw - 0.1,
    bezel,
  );

  const nameRows = textRows(CORE_NAMEPLATE);
  const nameW = (nameRows[0]?.length ?? 0) * NAMEPLATE_PX;
  const nameH = nameRows.length * NAMEPLATE_PX;
  const nameMid = (CORNICE_H0 + top) / 2;
  k.box(
    -nameW / 2 - NAMEPLATE_MARGIN,
    nameW / 2 + NAMEPLATE_MARGIN,
    CORNICE_D,
    CORNICE_D + NAMEPLATE_T,
    CORNICE_H0 + 0.01,
    top - 0.01,
    bezel,
  );
  pixelPanel(
    k,
    nameRows,
    -nameW / 2,
    nameMid + nameH / 2,
    NAMEPLATE_PX,
    CORNICE_D + NAMEPLATE_T + DECAL_LIFT,
    (ch) => (ch === "#" ? s.signal(LAMP_WHITE) : null),
  );
};

// --- The gun rack ----------------------------------------------------

/** The gun rack's backboard: its height band and its depth. */
const RACK_BACK_H0 = 0.7;
const RACK_BACK_H1 = 1.9;
const RACK_BACK_D = 0.04;

/** The gun's placement on the rack: its span along the wall, its back and front, its base. */
const RACK_GUN_A0 = -0.8;
const RACK_GUN_A1 = 0.85;
const RACK_GUN_D0 = 0.06;
const RACK_GUN_D1 = 0.28;
const RACK_GUN_H0 = 1.15;

/** How deep a cradle bracket reaches under the gun, and how tall it is. */
const CRADLE_FRONT = 0.25;
const CRADLE_TALL = 0.12;

/**
 * The gun rack: flush on its wall. A bare metal backboard (h `RACK_BACK_H0` to
 * `RACK_BACK_H1`, the width less 0.05 m each side) carries three rubber
 * cradle brackets at `gunRests`, and `bigGun` lies across them along `a`
 * from `RACK_GUN_A0` to `RACK_GUN_A1`, its grip hanging free between two
 * of them in front of the board. The gun's core blinks group 0 of the
 * breathe bank.
 */
const gunRack: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const bw = hw - 0.05;
  k.bevelBox(
    -bw,
    bw,
    0,
    RACK_BACK_D,
    RACK_BACK_H0,
    RACK_BACK_H1,
    0.01,
    s.metal,
  );
  for (const rest of gunRests(RACK_GUN_A0, RACK_GUN_A1, RACK_GUN_H0))
    k.bevelBox(
      rest.a - REST_HALF,
      rest.a + REST_HALF,
      RACK_BACK_D,
      CRADLE_FRONT,
      rest.top - CRADLE_TALL,
      rest.top,
      0.015,
      s.tinted(PAD_RUBBER),
    );
  bigGun(k, s, RACK_GUN_A0, RACK_GUN_A1, RACK_GUN_D0, RACK_GUN_D1, RACK_GUN_H0);
};

// --- The gun bench -----------------------------------------------------

/** The gun's placement on the bench: its span along the bench, its back and front. */
const BENCH_GUN_A0 = -0.5;
const BENCH_GUN_A1 = 0.9;
const BENCH_GUN_D0 = 0.32;
const BENCH_GUN_D1 = 0.54;
/** How far the gun's belly sits above the bench top: the grip's drop and a little air. */
const BENCH_GUN_LIFT = GRIP_DROP + 0.02;

/** The pegboard's tool silhouettes: their a-centres, heights and head widths. */
const BENCH_TOOLS: readonly {
  a: number;
  h0: number;
  h1: number;
  head: number;
}[] = [
  { a: -0.7, h0: 1.0, h1: 1.32, head: 0.05 },
  { a: -0.3, h0: 1.05, h1: 1.28, head: 0.035 },
  { a: 0.6, h0: 1.05, h1: 1.3, head: 0.06 },
];

/**
 * The gun bench: `workbench` at the catalogue surface's height, `bigGun`
 * lying over its right part (`BENCH_GUN_A0` to `BENCH_GUN_A1`) on three
 * rubber pads at `gunRests`, its belly `BENCH_GUN_LIFT` above the top so
 * the grip hangs clear, and a few tool silhouettes (a handle and a head
 * each) on the pegboard. The bench's left end, the catalogue's surface,
 * is left clear. The gun's core blinks group 0 of the breathe bank.
 */
const gunBench: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw, d1 } = heroHalf(kind, variant);
  const top = benchSurface(kind).h;
  workbench(k, s, hw, d1, top);
  const h0 = top + BENCH_GUN_LIFT;
  const benchGunD = (BENCH_GUN_D0 + BENCH_GUN_D1) / 2;
  for (const rest of gunRests(BENCH_GUN_A0, BENCH_GUN_A1, h0))
    k.bevelBox(
      rest.a - REST_HALF,
      rest.a + REST_HALF,
      benchGunD - GUN_DEPTH / 2 + 0.01,
      benchGunD + GUN_DEPTH / 2 - 0.01,
      top,
      rest.top,
      0.01,
      s.tinted(PAD_RUBBER),
    );
  bigGun(k, s, BENCH_GUN_A0, BENCH_GUN_A1, BENCH_GUN_D0, BENCH_GUN_D1, h0);
  for (const t of BENCH_TOOLS) {
    k.box(t.a - 0.012, t.a + 0.012, 0.03, 0.045, t.h0, t.h1, s.dark);
    k.box(
      t.a - t.head / 2,
      t.a + t.head / 2,
      0.03,
      0.05,
      t.h1 - 0.05,
      t.h1,
      s.metal,
    );
  }
};

// --- The tube bench ----------------------------------------------------

/** The tubes' glow: a warm amber, like gas-discharge tubes. */
const TUBE_AMBER: Rgb = [0.95, 0.55, 0.15];

/** The box's small label plate: a pale cream. */
const LABEL_CREAM: Rgb = [0.9, 0.85, 0.75];

/** The soldering lamp's bulb: a warm white filament. */
const LAMP_WARM: Rgb = [1.0, 0.92, 0.75];

/** The metal box holding the tube Y, in the recipe's local terms. */
const BOX_A0 = 0.1;
const BOX_A1 = 0.7;
const BOX_D0 = 0.3;
const BOX_D1 = 0.7;
const BOX_H1 = 1.4;
/** The box's wall thickness, and the front window frame's rail and stile widths. */
const BOX_WALL = 0.02;
const FRAME_RAIL = 0.06;
const FRAME_TOP_RAIL = 0.04;
const FRAME_STILE = 0.04;

/** The Y of tubes: its hub's centre in `(a, h)`, the hub's depth band and radius. */
const HUB_A = 0.4;
const HUB_H = 1.15;
const HUB_D0 = 0.58;
const HUB_D1 = 0.64;
const HUB_RADIUS = 0.045;
/** A tube's length from the hub's centre, its width and its depth band (inside the hub's). */
const TUBE_LEN = 0.19;
const TUBE_WIDTH = 0.028;
const TUBE_D0 = 0.6;
const TUBE_D1 = 0.62;

/** The Y's three arm directions in degrees: two arms up and out, the stem straight down. */
const TUBE_ANGLES_DEG: readonly number[] = [30, 150, 270];

/** The side door: its hinge at the box's back right corner, its width and how far it stands open. */
const DOOR_WIDTH = BOX_D1 - BOX_D0;
const DOOR_OPEN = (55 * Math.PI) / 180;

/** The soldering lamp: its base, its two jointed arms and the shade hanging from the second. */
const LAMP_A = -0.15;
const LAMP_BASE_D = 0.16;
const LAMP_BASE_T = 0.04;
const LAMP_ARM1_ANGLE = (80 * Math.PI) / 180;
const LAMP_ARM1_LEN = 0.95;
const LAMP_ARM2_ANGLE = (-35 * Math.PI) / 180;
const LAMP_ARM2_LEN = 0.34;
/** The shade's rim radius and its height from rim to apex. */
const SHADE_RADIUS = 0.1;
const SHADE_TALL = 0.13;

/** The cable coil on the lower shelf under the box: its centre and radius. */
const COIL_A = 0.4;
const COIL_D = 0.45;
const COIL_RADIUS = 0.07;

/**
 * The Y of three glowing tubes: a round dark hub (an extruded
 * `discOutline` facing the room) and three thin `tiltedBar` tubes in the
 * `(a, h)` plane at `TUBE_ANGLES_DEG`, each running from the hub's centre
 * out to its tip, so every tube reaches the hub. A thin dark rod runs back
 * from each tip and from the hub to the box's back plate, mounting them,
 * and three tiny sparks flicker on the hub's face.
 */
function tubeY(k: Kit, s: Surfaces): void {
  k.extrude(discOutline(HUB_A, HUB_H, HUB_RADIUS, 10), HUB_D0, HUB_D1, s.dark);
  const mount = (a: number, h: number) =>
    k.box(
      a - 0.01,
      a + 0.01,
      BOX_D0 + BOX_WALL,
      TUBE_D0,
      h - 0.01,
      h + 0.01,
      s.dark,
    );
  mount(HUB_A, HUB_H);
  TUBE_ANGLES_DEG.forEach((deg, i) => {
    const angle = (deg * Math.PI) / 180;
    const tipA = HUB_A + TUBE_LEN * Math.cos(angle);
    const tipH = HUB_H + TUBE_LEN * Math.sin(angle);
    const bar = tiltedBar(
      (HUB_A + tipA) / 2,
      (HUB_H + tipH) / 2,
      angle,
      TUBE_LEN,
      TUBE_WIDTH,
    );
    k.extrude(bar, TUBE_D0, TUBE_D1, s.blink(TUBE_AMBER, i));
    mount(tipA, tipH);
  });
  [
    [-0.02, 0.015, 3],
    [0.018, 0.01, 5],
    [0.0, -0.022, 7],
  ].forEach(([da = 0, dh = 0, group = 3]) =>
    k.panel(
      HUB_A + da - 0.006,
      HUB_A + da + 0.006,
      HUB_D1 + DECAL_LIFT,
      HUB_H + dh - 0.006,
      HUB_H + dh + 0.006,
      s.blink(SPARK_TINT, group),
    ),
  );
}

/**
 * The metal box: back, left side and top walls, a front frame round a
 * large open window with a small cream label on its bottom rail and five
 * status lights along its top rail (groups 3 to 7 of the chase bank), and
 * its right side a door hinged at the back corner, standing open, a darker
 * inner panel on its face.
 */
function tubeBox(k: Kit, kitAt: KitAt, s: Surfaces, h0: number): void {
  k.box(BOX_A0, BOX_A1, BOX_D0, BOX_D0 + BOX_WALL, h0, BOX_H1, s.metal);
  k.box(BOX_A0, BOX_A0 + BOX_WALL, BOX_D0, BOX_D1, h0, BOX_H1, s.metal);
  k.box(BOX_A0, BOX_A1, BOX_D0, BOX_D1, BOX_H1 - BOX_WALL, BOX_H1, s.metal);
  const fd0 = BOX_D1 - BOX_WALL;
  const topRail = BOX_H1 - BOX_WALL - FRAME_TOP_RAIL;
  k.box(BOX_A0, BOX_A1, fd0, BOX_D1, h0, h0 + FRAME_RAIL, s.metal);
  k.box(BOX_A0, BOX_A1, fd0, BOX_D1, topRail, BOX_H1 - BOX_WALL, s.metal);
  k.box(
    BOX_A0,
    BOX_A0 + FRAME_STILE,
    fd0,
    BOX_D1,
    h0 + FRAME_RAIL,
    topRail,
    s.metal,
  );
  k.box(
    BOX_A1 - FRAME_STILE,
    BOX_A1,
    fd0,
    BOX_D1,
    h0 + FRAME_RAIL,
    topRail,
    s.metal,
  );
  const am = (BOX_A0 + BOX_A1) / 2;
  k.panel(
    am - 0.09,
    am + 0.09,
    BOX_D1 + DECAL_LIFT,
    h0 + 0.015,
    h0 + 0.045,
    s.tinted(LABEL_CREAM),
  );
  STATUS_TINTS.forEach((tint, i) => {
    const a = am - 0.16 + i * 0.08;
    const hm = topRail + FRAME_TOP_RAIL / 2;
    k.panel(
      a - 0.012,
      a + 0.012,
      BOX_D1 + DECAL_LIFT,
      hm - 0.012,
      hm + 0.012,
      s.blink(tint, 3 + i),
    );
  });
  tubeY(k, s);

  const door = kitAt(yawed(ORIGIN, BOX_A1, BOX_D0, DOOR_OPEN));
  door.box(0, DOOR_WIDTH, -BOX_WALL, 0, h0 + 0.01, BOX_H1 - 0.01, s.metal);
  door.box(0.05, DOOR_WIDTH - 0.05, 0, 0.006, h0 + 0.08, BOX_H1 - 0.08, s.dark);
  for (const h of [h0 + 0.08, BOX_H1 - 0.12])
    k.cylinder(BOX_A1, BOX_D0, h, h + 0.04, 0.012, 6, s.dark);
}

/**
 * The soldering lamp: a weighted base, a long arm rising steeply to a
 * knuckle near the top, a short second arm reaching forward and down, and
 * a cone shade hanging from it, its wide rim at the bottom and its apex at
 * the arm (a `lathe` from the bottom up for the outside and from the top
 * down for the inside, so it reads from below too), with a socket and a
 * warm bulb inside the rim glowing down (`s.signal`).
 */
function solderingLamp(k: Kit, kitAt: KitAt, s: Surfaces, top: number): void {
  const baseTop = top + LAMP_BASE_T;
  k.bevelBox(
    LAMP_A - 0.06,
    LAMP_A + 0.06,
    LAMP_BASE_D - 0.08,
    LAMP_BASE_D + 0.08,
    top,
    baseTop,
    0.01,
    s.dark,
  );
  const j1d = LAMP_BASE_D + LAMP_ARM1_LEN * Math.cos(LAMP_ARM1_ANGLE);
  const j1h = baseTop + LAMP_ARM1_LEN * Math.sin(LAMP_ARM1_ANGLE);
  const j2d = j1d + LAMP_ARM2_LEN * Math.cos(LAMP_ARM2_ANGLE);
  const j2h = j1h + LAMP_ARM2_LEN * Math.sin(LAMP_ARM2_ANGLE);
  const arm = (
    d0: number,
    h0: number,
    d1: number,
    h1: number,
    angle: number,
    len: number,
  ) =>
    profileAlong(
      kitAt,
      ORIGIN,
      tiltedBar((d0 + d1) / 2, (h0 + h1) / 2, angle, len, 0.025),
      LAMP_A - 0.012,
      LAMP_A + 0.012,
      s.metal,
    );
  arm(LAMP_BASE_D, baseTop, j1d, j1h, LAMP_ARM1_ANGLE, LAMP_ARM1_LEN);
  arm(j1d, j1h, j2d, j2h, LAMP_ARM2_ANGLE, LAMP_ARM2_LEN);
  k.cylinderAlong(LAMP_A - 0.025, LAMP_A + 0.025, j1d, j1h, 0.02, 8, s.dark);

  const rim = j2h - SHADE_TALL;
  k.lathe(
    LAMP_A,
    j2d,
    [
      [SHADE_RADIUS, rim],
      [0.075, rim + 0.07],
      [0.03, j2h - 0.01],
      [0, j2h],
    ],
    10,
    s.dark,
  );
  k.lathe(
    LAMP_A,
    j2d,
    [
      [0, j2h - 0.012],
      [0.028, j2h - 0.022],
      [0.072, rim + 0.07],
      [SHADE_RADIUS - 0.004, rim],
    ],
    10,
    s.metal,
  );
  k.cylinder(LAMP_A, j2d, rim + 0.06, j2h - 0.01, 0.015, 6, s.dark);
  k.cylinder(
    LAMP_A,
    j2d,
    rim + 0.03,
    rim + 0.06,
    0.04,
    10,
    s.signal(LAMP_WARM),
  );
}

/**
 * The tube bench: `workbench` at the catalogue surface's height. On its
 * right part the metal box with its window, its open side door and the
 * tube Y inside (`tubeBox`); left of it a small tool box and a
 * screwdriver on the top; the soldering lamp (`solderingLamp`) reaching
 * over the middle; and a coil of cable lying on the lower shelf with its
 * lead rising into the slab under the box. The bench's left end, the
 * catalogue's surface, is left clear. Lights: the tubes are groups 0 to
 * 2 of the chase bank, the box's status lights and the sparks groups 3 to
 * 7, the lamp's bulb steady.
 */
const tubeBench: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, d1 } = heroHalf(kind, variant);
  const top = benchSurface(kind).h;
  workbench(k, s, hw, d1, top);
  tubeBox(k, kitAt, s, top);
  solderingLamp(k, kitAt, s, top);

  k.bevelBox(-0.05, 0.07, 0.72, 0.84, top, top + 0.06, 0.008, s.dark);
  k.box(-0.26, -0.12, 0.655, 0.675, top, top + 0.015, s.metal);
  k.box(-0.12, -0.06, 0.65, 0.68, top, top + 0.028, s.tinted(STATUS_RED));

  for (const h of [SHELF_H1 + 0.012, SHELF_H1 + 0.036])
    k.ring(COIL_A, COIL_D, h, COIL_RADIUS, 0.012, 3, 8, s.dark, "up");
  k.cylinder(
    COIL_A + COIL_RADIUS,
    COIL_D,
    SHELF_H1 + 0.03,
    top - SLAB_THICK,
    0.01,
    6,
    s.dark,
  );
};

// --- The field pack ----------------------------------------------------

/** The pack's shell: a matte black. */
const PACK_BLACK: Rgb = [0.06, 0.06, 0.07];

/** The pack's frame, the cyclotron and the wand's barrel: a bare aluminium grey. */
const FRAME_GREY: Rgb = [0.55, 0.55, 0.57];

/** The wand's body: a mid grey. */
const WAND_GREY: Rgb = [0.42, 0.43, 0.45];

/** The cyclotron's four lights: a deep red behind red lenses. */
const CYCLOTRON_RED: Rgb = [0.75, 0.08, 0.08];

/** The power cell's column of lights: a clear blue. */
const CELL_BLUE: Rgb = [0.15, 0.35, 0.85];

/** The wand's tip: a small orange light. */
const TIP_ORANGE: Rgb = [0.95, 0.6, 0.2];

/** The pack body's size: width, depth (back to front) and height. */
const PACK_W = 0.4;
const PACK_D = 0.24;
const PACK_H = 0.68;

/** The frame's bar side and its bottom plate's thickness. */
const FRAME_T = 0.03;
const PLATE_H = 0.025;
/**
 * Where the frame's uprights stand along `a`: behind the body, a little
 * wider than it, so the grey frame shows past both its sides.
 */
const FRAME_A = PACK_W / 2 + 0.005;
/** How far the frame's uprights rise over the body's top. */
const FRAME_OVER = 0.02;

/** How far v0's body top leans back of its bottom, in metres. */
const PACK_LEAN = 0.08;

/**
 * v1's frame plate's bottom: the pack stands on its rack so that the
 * frame's top reaches the catalogue top, 1.6.
 */
const V1_PLATE_H0 = 1.6 - FRAME_OVER - PACK_H - PLATE_H;

/** The rack's deck and its posts (v1). */
const RACK_DECK_T = 0.03;
const RACK_HALF_A = 0.3;
const RACK_HALF_D = 0.22;
const RACK_POST_A = 0.28;
const RACK_POST_D = 0.2;
const RACK_RAIL_H = 0.25;

/** The wand's length, gunbox to tip, and where its axis stands off the body's left side. */
const WAND_LEN = 0.5;
const WAND_OFF = 0.065;
/** v0's wand stand: the wand's bottom height, its axis's depth and the base disc's radius. */
const V0_WAND_BASE = 1.1;
const V0_WAND_D = 0.05;
const V0_STAND_RADIUS = 0.09;

/** The ribbed hose's radius, its ribs' radius and their spacing along the hose. */
const HOSE_RADIUS = 0.02;
const RIB_RADIUS = 0.027;
const RIB_PITCH = 0.06;

/**
 * The field pack's shape at one variant: where the body's bottom is, how
 * far it leans back at a height, and so the depth of its front and back
 * faces at any height (`front(h)`, `back(h)`), which every part built
 * against a face reads so it touches it exactly where it is built.
 */
interface PackPose {
  plate: number;
  bottom: number;
  top: number;
  lean(h: number): number;
  front(h: number): number;
  back(h: number): number;
}

/** The pose of variant `variant`: v0 on the floor leaning back, v1 upright on its rack. */
function packPose(variant: number): PackPose {
  const plate = variant === 0 ? 0 : V1_PLATE_H0;
  const bottom = plate + PLATE_H;
  const lean = (h: number) =>
    variant === 0 ? (PACK_LEAN * (h - bottom)) / PACK_H : 0;
  return {
    plate,
    bottom,
    top: bottom + PACK_H,
    lean,
    front: (h) => PACK_D / 2 - lean(h),
    back: (h) => -PACK_D / 2 - lean(h),
  };
}

/**
 * The body and its own grey frame: the black body (an upright bevelled
 * box, or v0's leaning parallelogram through `profileAlong`) standing on
 * the frame's bottom plate, two uprights behind its back following its
 * lean, a top bar joining them over the body and a middle bar.
 */
function packBody(k: Kit, kitAt: KitAt, s: Surfaces, p: PackPose): void {
  const shell = s.tinted(PACK_BLACK);
  const grey = s.tinted(FRAME_GREY);
  if (p.lean(p.top) === 0)
    k.bevelBox(
      -PACK_W / 2,
      PACK_W / 2,
      -PACK_D / 2,
      PACK_D / 2,
      p.bottom,
      p.top,
      HOUSING_BEVEL,
      shell,
    );
  else
    profileAlong(
      kitAt,
      ORIGIN,
      [
        [p.back(p.bottom), p.bottom],
        [p.front(p.bottom), p.bottom],
        [p.front(p.top), p.top],
        [p.back(p.top), p.top],
      ],
      -PACK_W / 2,
      PACK_W / 2,
      shell,
    );
  k.box(
    -PACK_W / 2 - 0.01,
    PACK_W / 2 + 0.01,
    p.back(p.bottom) - FRAME_T,
    p.front(p.bottom) - 0.02,
    p.plate,
    p.bottom,
    grey,
  );
  const frameTop = p.top + FRAME_OVER;
  const strip = (h0: number, h1: number, a0: number, a1: number) =>
    profileAlong(
      kitAt,
      ORIGIN,
      [
        [p.back(h0) - FRAME_T, h0],
        [p.back(h0), h0],
        [p.back(h1), h1],
        [p.back(h1) - FRAME_T, h1],
      ],
      a0,
      a1,
      grey,
    );
  const ua = FRAME_A;
  for (const a of [-ua, ua])
    strip(p.bottom, frameTop, a - FRAME_T / 2, a + FRAME_T / 2);
  strip(frameTop - FRAME_T, frameTop, -ua - FRAME_T / 2, ua + FRAME_T / 2);
  const mid = p.bottom + PACK_H * 0.45;
  strip(mid, mid + FRAME_T, -ua, ua);
}

/**
 * The details on the body's front face (the side that faces the room):
 * the round cyclotron low on it (a grey disc, a black centre and four red
 * lenses at its quarters, chase groups 0 to 3 in order round the ring),
 * the power-cell housing above it on the left with a column of eight blue
 * lights (chase groups 4 to 7, bottom to top, twice), two ribbed booster
 * tubes lying across the face above the cyclotron, a black cable along
 * the top, and a bent carrying arm on the right side.
 */
function packFace(k: Kit, kitAt: KitAt, s: Surfaces, p: PackPose): void {
  const grey = s.tinted(FRAME_GREY);
  const black = s.tinted(PACK_BLACK);

  const cycA = 0.04;
  const cycH = p.bottom + 0.15;
  const cycR = 0.115;
  const cycD0 = p.front(cycH + cycR) - 0.004;
  const cycD1 = p.front(cycH) + 0.05;
  k.extrude(discOutline(cycA, cycH, cycR, 12), cycD0, cycD1, grey);
  k.extrude(discOutline(cycA, cycH, 0.075, 10), cycD1, cycD1 + 0.008, black);
  [45, 315, 225, 135].forEach((deg, i) => {
    const t = (deg * Math.PI) / 180;
    const la = cycA + 0.095 * Math.cos(t);
    const lh = cycH + 0.095 * Math.sin(t);
    k.extrude(
      discOutline(la, lh, 0.02, 8),
      cycD1,
      cycD1 + 0.014,
      s.blink(CYCLOTRON_RED, i),
    );
  });

  const cellA0 = -0.17;
  const cellA1 = -0.1;
  const cellH0 = p.bottom + 0.3;
  const cellH1 = p.bottom + 0.62;
  const cellT = 0.03;
  profileAlong(
    kitAt,
    ORIGIN,
    [
      [p.front(cellH0) - 0.004, cellH0],
      [p.front(cellH0) + cellT, cellH0],
      [p.front(cellH1) + cellT, cellH1],
      [p.front(cellH1) - 0.004, cellH1],
    ],
    cellA0,
    cellA1,
    s.dark,
  );
  const cells = 8;
  const pitch = (cellH1 - cellH0 - 0.02) / cells;
  for (let i = 0; i < cells; i++) {
    const h = cellH0 + 0.01 + i * pitch;
    const d = p.front(h + pitch / 2) + cellT + DECAL_LIFT;
    k.panel(
      cellA0 + 0.015,
      cellA1 - 0.015,
      d,
      h + 0.006,
      h + pitch - 0.006,
      s.blink(CELL_BLUE, 4 + (i % 4)),
    );
  }

  for (const [h, r, a0, a1] of [
    [p.bottom + 0.34, 0.035, -0.06, 0.17],
    [p.bottom + 0.43, 0.028, -0.03, 0.14],
  ] as const) {
    const d = p.front(h) + r - 0.012;
    k.cylinderAlong(a0, a1, d, h, r, 8, grey);
    for (const a of [a0 + 0.04, (a0 + a1) / 2, a1 - 0.04])
      k.cylinderAlong(a - 0.008, a + 0.008, d, h, r + 0.009, 6, s.dark);
  }
  const cableH = p.bottom + 0.64;
  k.cylinderAlong(
    -0.14,
    0.17,
    p.front(cableH) + 0.004,
    cableH,
    0.012,
    6,
    black,
  );

  const armLow = p.bottom + 0.22;
  const armHigh = p.bottom + 0.52;
  const armOut = PACK_W / 2 + 0.07;
  for (const h of [armLow, armHigh])
    k.cylinderAlong(
      PACK_W / 2 - 0.01,
      armOut + 0.012,
      -p.lean(h),
      h,
      0.012,
      6,
      grey,
    );
  k.cylinder(
    armOut,
    -p.lean((armLow + armHigh) / 2),
    armLow,
    armHigh,
    0.012,
    6,
    grey,
  );
}

/**
 * The wand, standing tip up with its bottom at `base` on the axis `(a,
 * d)`: a grey gunbox with a black grip jutting forward, a finned dark
 * heat-sink collar, a slim grey barrel and a small orange tip (chase
 * group 7), and a dark socket under the gunbox where the hose plugs in.
 */
function wand(k: Kit, s: Surfaces, a: number, d: number, base: number): void {
  k.box(
    a - 0.035,
    a + 0.035,
    d - 0.045,
    d + 0.045,
    base,
    base + 0.18,
    s.tinted(WAND_GREY),
  );
  k.box(
    a - 0.018,
    a + 0.018,
    d + 0.045,
    d + 0.1,
    base + 0.02,
    base + 0.09,
    s.tinted(PACK_BLACK),
  );
  k.cylinder(a, d, base + 0.18, base + 0.23, 0.032, 8, s.dark);
  for (const h of [base + 0.19, base + 0.212])
    k.cylinder(a, d, h, h + 0.008, 0.042, 6, s.dark);
  k.cylinder(
    a,
    d,
    base + 0.23,
    base + WAND_LEN - 0.02,
    0.017,
    8,
    s.tinted(FRAME_GREY),
  );
  k.cylinder(
    a,
    d,
    base + WAND_LEN - 0.02,
    base + WAND_LEN,
    0.021,
    8,
    s.blink(TIP_ORANGE, 7),
  );
  k.cylinder(a, d, base - 0.04, base, 0.022, 6, s.dark);
}

/**
 * The ribbed hose from the body's left side to the wand's socket: a
 * straight run out of the body along `a` at `hSide`, an elbow, a ribbed
 * run up or down beside the wand at depth `dHose` to `hWand`, an elbow,
 * and a short run along `d` into the socket at `(aWand, dWand)`. Every
 * piece starts where the last ends, so the hose is one connected run.
 */
function ribbedHose(
  k: Kit,
  kitAt: KitAt,
  s: Surfaces,
  aWand: number,
  dWand: number,
  dHose: number,
  hSide: number,
  hWand: number,
): void {
  const hose = s.tinted(PACK_BLACK);
  k.cylinderAlong(
    aWand,
    -PACK_W / 2 + 0.02,
    dHose,
    hSide,
    HOSE_RADIUS,
    6,
    hose,
  );
  const [h0, h1] = hSide < hWand ? [hSide, hWand] : [hWand, hSide];
  k.cylinder(
    aWand,
    dHose,
    h0 - HOSE_RADIUS,
    h1 + HOSE_RADIUS,
    HOSE_RADIUS,
    6,
    hose,
  );
  const ribs = Math.floor((h1 - h0 - 0.04) / RIB_PITCH);
  for (let i = 1; i <= ribs; i++) {
    const h = h0 + 0.02 + i * RIB_PITCH - RIB_PITCH / 2;
    k.cylinder(
      aWand,
      dHose,
      h - 0.008,
      h + 0.008,
      RIB_RADIUS,
      6,
      s.dark,
      false,
    );
  }
  kitAt(yawed(ORIGIN, aWand, 0, Math.PI / 2)).cylinderAlong(
    dHose,
    dWand,
    0,
    hWand,
    HOSE_RADIUS,
    6,
    hose,
  );
}

/**
 * v0's stand: two struts from the frame's uprights, low on its back, down
 * behind it to the floor, a foot pad each, so the leaning pack is propped;
 * and the wand's own floor stand beside the pack (a base disc and a thin
 * post up to the wand).
 */
function packV0Stands(
  k: Kit,
  kitAt: KitAt,
  s: Surfaces,
  p: PackPose,
  aWand: number,
): void {
  const footH = 0.03;
  const footD = p.back(0) - FRAME_T - 0.16;
  const joinH = p.bottom + 0.35;
  const joinD = p.back(joinH) - FRAME_T;
  const ua = FRAME_A;
  for (const a of [-ua, ua]) {
    profileAlong(
      kitAt,
      ORIGIN,
      tiltedBar(
        (joinD + footD) / 2,
        (joinH + footH) / 2,
        Math.atan2(joinH - footH, joinD - footD),
        Math.hypot(joinH - footH, joinD - footD),
        0.025,
      ),
      a - FRAME_T / 2,
      a + FRAME_T / 2,
      s.tinted(FRAME_GREY),
    );
    k.cylinder(a, footD, 0, footH, 0.025, 8, s.dark);
  }
  k.cylinder(aWand, V0_WAND_D, 0, 0.02, V0_STAND_RADIUS, 8, s.dark);
  k.cylinder(aWand, V0_WAND_D, 0.02, V0_WAND_BASE - 0.04, 0.012, 6, s.metal);
}

/**
 * v1's rack: a deck the frame's plate stands on, four corner posts
 * (`cornerPosts`) down to the floor and two low rails joining them along
 * `a`; and the gun mount, a ribbed grey clip on the body's left side that
 * the wand hangs in.
 */
function packV1Rack(
  k: Kit,
  s: Surfaces,
  p: PackPose,
  aWand: number,
  wandBase: number,
): void {
  const deck0 = p.plate - RACK_DECK_T;
  k.box(
    -RACK_HALF_A,
    RACK_HALF_A,
    -RACK_HALF_D,
    RACK_HALF_D,
    deck0,
    p.plate,
    s.metal,
  );
  cornerPosts(
    k,
    [-RACK_POST_A, RACK_POST_A],
    [-RACK_POST_D, RACK_POST_D],
    0.02,
    0,
    deck0,
    s.metal,
  );
  for (const d of [-RACK_POST_D, RACK_POST_D])
    k.box(
      -RACK_POST_A,
      RACK_POST_A,
      d - 0.015,
      d + 0.015,
      RACK_RAIL_H,
      RACK_RAIL_H + 0.03,
      s.metal,
    );
  const mount0 = -PACK_W / 2;
  const mount1 = aWand + 0.035;
  const grey = s.tinted(FRAME_GREY);
  k.box(mount1, mount0, -0.06, 0.06, wandBase + 0.02, wandBase + 0.16, grey);
  for (const h of [wandBase + 0.04, wandBase + 0.085, wandBase + 0.13])
    k.box(mount1 - 0.006, mount0, -0.065, 0.065, h, h + 0.012, s.dark);
}

/**
 * The field pack: a matte black backpack unit on a grey metal frame of
 * its own (`packBody`), its front face carrying the cyclotron, the power
 * cell, the booster tubes, a cable and a carrying arm (`packFace`), and a
 * grey wand with a black grip and an orange tip (`wand`) joined to the
 * pack's left side by a ribbed black hose (`ribbedHose`).
 *
 * v0: the frame's plate on the floor, the body leaning back `PACK_LEAN`
 * on two struts, the wand standing upright on its own floor stand beside
 * the pack, its tip at the catalogue top (1.6), the hose climbing from
 * the pack to it. v1: upright on a four-post rack (`packV1Rack`), the
 * frame's top at the catalogue top, the wand clipped into a ribbed mount
 * on the body's side, the hose dropping from high on the side to its
 * socket.
 *
 * The kind's bank is chase, which lights one group at a time in order:
 * the light runs round the cyclotron's ring (groups 0 to 3), then climbs
 * the power cell's column (groups 4 to 7, both halves of the column
 * together), and the wand's tip flashes with the column's top step.
 */
const fieldPack: HeroRecipe = ({ k, kitAt, s, variant }) => {
  const p = packPose(variant);
  const aWand = -PACK_W / 2 - WAND_OFF - 0.035;
  packBody(k, kitAt, s, p);
  packFace(k, kitAt, s, p);
  if (variant === 0) {
    const base = V0_WAND_BASE;
    packV0Stands(k, kitAt, s, p, aWand);
    wand(k, s, aWand, V0_WAND_D, base);
    ribbedHose(
      k,
      kitAt,
      s,
      aWand,
      V0_WAND_D,
      V0_WAND_D - 0.08,
      p.bottom + 0.4,
      base - 0.04,
    );
  } else {
    const base = p.bottom + 0.1;
    packV1Rack(k, s, p, aWand, base);
    wand(k, s, aWand, 0, base);
    ribbedHose(k, kitAt, s, aWand, 0, -0.1, p.top - 0.08, base - 0.04);
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
