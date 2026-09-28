/**
 * The wall props' recipes: everything that hangs or stands flush against a
 * wall edge, and the two wall runs.
 *
 * A wall prop stays within `WALL_REACH` of its anchor along the wall,
 * within `FLUSH_DEPTH` of the wall and between the floor and `WALL_TOP`; a
 * run segment spans the whole edge (`RUN_REACH`) in `RUN_BAND`, above
 * every wall prop. Every recipe is built at turn 0 on a wall at `d = 0`,
 * facing `+d`, so it works purely in the kit's local `(a, d, h)` terms and
 * never sees the room; the prop test builds every kind, variant and turn
 * against those bounds.
 *
 * The locker bank carries the room's accent (`s.accent()`, 2.7 C9) on the
 * topmost vent slit of every door, its trim strip, in every variant. No
 * other wall prop carries it.
 *
 * In a look with `propAccents` a prop paints exactly one part in its own
 * accent instead (`s.own`) and the slits turn dark metal: one locker handle,
 * the breaker box's lever (variant 0) or the bar across its two boxes
 * (variant 1) and a trim along the conduit cabinet's top. A
 * part only that look draws is added only there.
 */

import { FLAG, type Surface } from "../../geometry";
import { DECAL_LIFT, frameAt, type Kit } from "../../kit";
import { LAYER } from "../../layers";
import type { Rgb } from "../../looks";
import { PICTOGRAM, SIGN_PICTOGRAMS } from "../../text";
import type { RarePropKind } from "../../../world/props";
import type { WallPropKind } from "../../../world/types";
import { discOutline, shade, tiltedBar, yawed } from "../common";
import { RUN_BAND, type PropRecipe } from "./common";

/** A warm hazard red: the extinguisher's cylinder and the first-aid cross. */
const RED: Rgb = [0.74, 0.09, 0.07];

/** A status green: the keycard reader's ready glow, and the pipe riser's gauge face. */
const GREEN: Rgb = [0.22, 0.92, 0.34];

/** A muted upholstery tan: the padded panel's cushions. */
const QUILT: Rgb = [0.55, 0.5, 0.46];

/** The locker bank: its overall box and how tall its doors run. */
const LOCKER = { half: 0.85, depth: 0.26, h1: 2.0 };

/**
 * Locker bank: a cabinet of narrow doors, each with a vent slit and a
 * handle. Variant 0 has three doors, variant 1 four narrower ones. Every
 * door's topmost slit carries the room's accent, standing in for the trim
 * strip along its top (2.7 C9); its lower two slits stay plain dark metal.
 */
function lockerBank({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const L = LOCKER;
  const doors = variant === 0 ? 3 : 4;
  k.bevelBox(-L.half, L.half, 0, L.depth, 0, L.h1, 0.02, s.body);
  const inner = L.half - 0.03;
  const gap = 0.015;
  const width = (2 * inner - gap * (doors - 1)) / doors;
  for (let i = 0; i < doors; i++) {
    const a0 = -inner + i * (width + gap);
    const a1 = a0 + width;
    k.box(a0, a1, L.depth - 0.03, L.depth - 0.005, 0.05, L.h1 - 0.05, s.panel);
    for (let j = 0; j < 3; j++) {
      const h0 = L.h1 - 0.2 - j * 0.07;
      k.box(
        a0 + 0.04,
        a1 - 0.04,
        L.depth - 0.005,
        L.depth,
        h0,
        h0 + 0.02,
        j === 0 ? s.accent() : s.dark,
      );
    }
    k.box(
      a1 - 0.06,
      a1 - 0.03,
      L.depth - 0.005,
      L.depth + 0.03,
      L.h1 / 2 - 0.09,
      L.h1 / 2 + 0.09,
      i === 1 ? s.own(s.metal) : s.metal,
    );
  }
}

/** The extinguisher: its band on the wall and the cylinder's radius. */
const EXT = { h0: 0.35, h1: 1.15, radius: 0.09 };

/**
 * Extinguisher: variant 0 a red cylinder held to the wall by an open
 * bracket, variant 1 a cabinet with a window showing the cylinder inside.
 */
function extinguisher({ k, s, ctx, variant }: Parameters<PropRecipe>[0]): void {
  const E = EXT;
  if (variant === 0) {
    k.box(-0.12, 0.12, 0, 0.02, E.h0 - 0.05, E.h1 + 0.12, s.dark);
    k.cylinder(0, E.radius + 0.02, E.h0, E.h1, E.radius, 10, s.tinted(RED));
    k.cylinder(
      0,
      E.radius + 0.02,
      E.h1,
      E.h1 + 0.06,
      E.radius * 0.55,
      8,
      s.dark,
    );
    k.box(-0.03, 0.03, 0.03, 0.07, E.h1 + 0.06, E.h1 + 0.14, s.metal);
    for (const h of [E.h0 + 0.05, E.h1 - 0.1]) {
      k.cylinderAlong(-0.1, 0.1, 0.02, h, 0.015, 6, s.metal);
    }
  } else {
    const d = 0.22;
    const c0 = E.h0 - 0.15;
    const c1 = E.h1 + 0.2;
    k.bevelBox(-0.22, 0.22, 0, d, c0, c1, 0.015, s.body);
    k.box(
      -0.15,
      0.15,
      d - 0.03,
      d - 0.005,
      c0 + 0.1,
      c1 - 0.1,
      s.tinted(ctx.look.palette.screen),
    );
    k.cylinder(0, d * 0.4, E.h0, E.h1, E.radius * 0.85, 10, s.tinted(RED));
    k.box(-0.05, 0.05, d - 0.02, d, c1 - 0.08, c1 - 0.02, s.dark);
  }
}

/** The first-aid cabinet: its width and depth. */
const FIRST_AID = { half: 0.22, depth: 0.14 };

/**
 * First-aid cabinet: a small white box with a cross of two boxes on its
 * face. Variant 1 is taller, with a lower drawer.
 */
function firstAid({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const F = FIRST_AID;
  const body0 = variant === 0 ? 1.05 : 0.95;
  const body1 = variant === 0 ? 1.55 : 1.85;
  const cabinet = s.tinted([0.94, 0.94, 0.92]);
  k.bevelBox(-F.half, F.half, 0, F.depth, body0, body1, 0.015, cabinet);
  const mid = (body0 + body1) / 2;
  k.box(
    -0.016,
    0.016,
    F.depth - 0.005,
    F.depth + 0.008,
    mid - 0.11,
    mid + 0.11,
    s.tinted(RED),
  );
  k.box(
    -0.11,
    0.11,
    F.depth - 0.005,
    F.depth + 0.008,
    mid - 0.016,
    mid + 0.016,
    s.tinted(RED),
  );
  k.box(
    -0.03,
    0.03,
    F.depth - 0.02,
    F.depth - 0.005,
    body0 + 0.03,
    body0 + 0.08,
    s.dark,
  );
  if (variant === 1) {
    const top = body0 - 0.03;
    const bottom = top - 0.22;
    k.bevelBox(-F.half, F.half, 0, F.depth - 0.01, bottom, top, 0.01, s.metal);
    const dh = (bottom + top) / 2;
    k.box(
      -0.05,
      0.05,
      F.depth - 0.02,
      F.depth - 0.005,
      dh - 0.008,
      dh + 0.008,
      s.dark,
    );
  }
}

/** The intercom panel: its width, depth and height band. */
const INTERCOM = { half: 0.12, depth: 0.08, h0: 1.3, h1: 1.65 };

/**
 * Intercom: variant 0 a panel with a slit grille and a call button,
 * variant 1 a handset resting on a cradle hook.
 */
function intercom({ k, s, ctx, variant }: Parameters<PropRecipe>[0]): void {
  const I = INTERCOM;
  k.bevelBox(-I.half, I.half, 0, I.depth, I.h0, I.h1, 0.012, s.metal);
  if (variant === 0) {
    for (let i = 0; i < 5; i++) {
      const h = I.h0 + 0.05 + i * 0.045;
      k.box(
        -0.08,
        0.08,
        I.depth - 0.008,
        I.depth + 0.004,
        h,
        h + 0.015,
        s.dark,
      );
    }
    k.box(
      -0.025,
      0.025,
      I.depth - 0.006,
      I.depth + 0.006,
      I.h0 + 0.02,
      I.h0 + 0.06,
      s.glow(ctx.look.palette.door),
    );
  } else {
    const hh = (I.h0 + I.h1) / 2;
    const d = I.depth + 0.05;
    k.box(-0.02, 0.02, I.depth, d, I.h0 + 0.05, I.h0 + 0.08, s.dark);
    k.cylinderAlong(-0.16, 0.16, d, hh, 0.02, 8, s.dark);
    k.cylinder(-0.16, d, hh - 0.035, hh + 0.035, 0.032, 8, s.dark);
    k.cylinder(0.16, d, hh - 0.035, hh + 0.035, 0.032, 8, s.dark);
  }
}

/** The keycard reader: its width, depth and where its band starts. */
const READER = { half: 0.08, depth: 0.06, h0: 1.0 };

/**
 * Keycard reader: a slim box with a card slot and a green status panel,
 * `DECAL_LIFT` off its face. Variant 1 adds a keypad below the slot.
 */
function keycardReader({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const R = READER;
  const h1 = variant === 0 ? R.h0 + 0.3 : R.h0 + 0.55;
  k.bevelBox(-R.half, R.half, 0, R.depth, R.h0, h1, 0.008, s.dark);
  k.box(
    -0.045,
    0.045,
    R.depth - 0.01,
    R.depth + 0.002,
    h1 - 0.08,
    h1 - 0.07,
    s.tinted([0.02, 0.02, 0.02]),
  );
  k.panel(
    -0.035,
    0.035,
    R.depth + DECAL_LIFT,
    h1 - 0.16,
    h1 - 0.12,
    s.glow(GREEN),
  );
  if (variant === 1) {
    for (let r = 0; r < 4; r++) {
      for (let c = 0; c < 3; c++) {
        const a0 = -0.06 + c * 0.045;
        const kh0 = R.h0 + 0.04 + r * 0.045;
        k.box(
          a0,
          a0 + 0.025,
          R.depth - 0.004,
          R.depth + 0.006,
          kh0,
          kh0 + 0.025,
          s.metal,
        );
      }
    }
  }
}

/** The vent grille: its width, depth and height band. */
const VENT = { half: 0.28, depth: 0.05, h0: 1.1, h1: 1.66 };

/**
 * Vent grille: variant 0 a fan hub with six radiating blades, variant 1
 * rows of louvres stepped forward towards the top.
 */
function ventGrille({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const V = VENT;
  k.bevelBox(-V.half, V.half, 0, V.depth, V.h0, V.h1, 0.01, s.dark);
  if (variant === 0) {
    const hc = (V.h0 + V.h1) / 2;
    for (let i = 0; i < 6; i++) {
      const angle = (Math.PI / 3) * i;
      const cx = Math.cos(angle) * 0.11;
      const cy = hc + Math.sin(angle) * 0.11;
      k.extrude(
        tiltedBar(cx, cy, angle, 0.15, 0.045),
        V.depth - 0.022,
        V.depth - 0.006,
        s.metal,
      );
    }
    k.box(
      -0.03,
      0.03,
      V.depth - 0.02,
      V.depth - 0.004,
      hc - 0.03,
      hc + 0.03,
      s.dark,
    );
  } else {
    const inner = V.half - 0.04;
    const rows = 3;
    for (let i = 0; i < rows; i++) {
      const cy = V.h0 + 0.14 + i * ((V.h1 - V.h0 - 0.28) / (rows - 1));
      k.extrude(
        tiltedBar(0, cy, 0.3, 2 * inner, 0.045),
        V.depth - 0.02,
        V.depth - 0.004,
        s.metal,
      );
    }
  }
}

/** The sign plate: its size (square, so its sign shows undistorted) and band. */
const SIGN = { half: 0.15, depth: 0.025, h0: 1.5, h1: 1.8 };

/**
 * Sign plate: a dark plate with a pictogram panel on `LAYER.pictogram`,
 * flush with the plate's face and lifted `DECAL_LIFT` in front of it
 * (ruling 21). Variant `v` shows `PICTOGRAM[SIGN_PICTOGRAMS[v]]`, one sign
 * per variant.
 */
function signPlate({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const P = SIGN;
  const key = SIGN_PICTOGRAMS[variant];
  if (key === undefined) {
    throw new Error(`sign-plate: no pictogram for variant ${String(variant)}`);
  }
  const rect = PICTOGRAM[key];
  k.bevelBox(-P.half, P.half, 0, P.depth, P.h0, P.h1, 0.01, s.dark);
  k.panel(
    -P.half + 0.02,
    P.half - 0.02,
    P.depth + DECAL_LIFT,
    P.h0 + 0.02,
    P.h1 - 0.02,
    { layer: LAYER.pictogram, tint: [1, 1, 1], flag: FLAG.emissive },
    rect.uw,
    rect.vh,
    rect.u0,
    rect.v0,
  );
}

/** The breaker box: its width, depth and height band. */
const BREAKER = { half: 0.22, depth: 0.12, h0: 1.1, h1: 1.7 };

/**
 * Breaker box: variant 0 a grey box with a hazard strip and a toggle
 * lever, variant 1 a double box joined by a conduit above.
 */
function breakerBox({ k, s, kitAt, variant }: Parameters<PropRecipe>[0]): void {
  const B = BREAKER;
  if (variant === 0) {
    k.bevelBox(-B.half, B.half, 0, B.depth, B.h0, B.h1, 0.012, s.metal);
    k.box(
      -B.half,
      B.half,
      B.depth - 0.01,
      B.depth + 0.002,
      B.h1 - 0.05,
      B.h1 - 0.03,
      s.hazard,
    );
    const mid = (B.h0 + B.h1) / 2;
    k.box(
      B.half - 0.02,
      B.half + 0.01,
      B.depth - 0.02,
      B.depth,
      mid - 0.1,
      mid + 0.1,
      s.dark,
    );
    const f = frameAt([0, 0, 0], 0);
    const lever = yawed(f, B.half - 0.05, B.depth, -0.7);
    kitAt(lever).box(
      -0.008,
      0.008,
      -0.01,
      0.09,
      mid - 0.02,
      mid + 0.02,
      s.own(s.dark),
    );
  } else {
    const w = B.half * 0.9;
    for (const cx of [-B.half - 0.02, B.half + 0.02]) {
      k.bevelBox(cx - w, cx + w, 0, B.depth, B.h0, B.h1, 0.012, s.metal);
      k.box(
        cx - w,
        cx + w,
        B.depth - 0.01,
        B.depth + 0.002,
        B.h1 - 0.05,
        B.h1 - 0.03,
        s.hazard,
      );
    }
    k.box(
      -B.half - 0.02 - w,
      B.half + 0.02 + w,
      B.depth - 0.02,
      B.depth,
      B.h1 + 0.02,
      B.h1 + 0.05,
      s.own(s.dark),
    );
  }
}

/** The wall monitor: its height band and depth, plain glow, no text. */
const MONITOR = { depth: 0.05, h0: 1.35, h1: 1.75 };

/**
 * Wall monitor: a small bezel with an emissive screen, `DECAL_LIFT` off
 * its face. Variant 1 is a wide screen.
 */
function wallMonitor({ k, s, ctx, variant }: Parameters<PropRecipe>[0]): void {
  const M = MONITOR;
  const half = variant === 0 ? 0.22 : 0.4;
  k.bevelBox(-half, half, 0, M.depth, M.h0, M.h1, 0.012, s.dark);
  k.panel(
    -half + 0.02,
    half - 0.02,
    M.depth + DECAL_LIFT,
    M.h0 + 0.02,
    M.h1 - 0.02,
    s.glow(ctx.look.palette.screenText),
  );
}

/** The padded panel: its overall size and cushion depth, widened to D4's reach. */
const PADDED = { half: 0.85, h0: 0.6, h1: 2.0, depth: 0.06 };

/**
 * Padded panel: a quilted wall of bevelled cushions, 4 by 3 in variant 0
 * and 5 by 4 in variant 1, wide enough to reach `WIDE_REACH` on both sides
 * of its anchor (D4).
 */
function paddedPanel({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const P = PADDED;
  const cols = variant === 0 ? 4 : 5;
  const rows = variant === 0 ? 3 : 4;
  const gap = 0.01;
  const cw = (2 * P.half - gap * (cols - 1)) / cols;
  const rh = (P.h1 - P.h0 - gap * (rows - 1)) / rows;
  const bevel = Math.min(0.015, cw * 0.2, rh * 0.2);
  const quilt = s.tinted(QUILT);
  for (let c = 0; c < cols; c++) {
    for (let r = 0; r < rows; r++) {
      const a0 = -P.half + c * (cw + gap);
      const h0 = P.h0 + r * (rh + gap);
      k.bevelBox(a0, a0 + cw, 0, P.depth, h0, h0 + rh, bevel, quilt);
    }
  }
}

/** The light strip housing: its height band, under `WALL_TOP`. */
const LIGHT_STRIP = { h0: 0.3, h1: 2.2 };

/**
 * Light strip: a vertical emissive bar in a thin housing. Variant 1 is two
 * bars side by side.
 */
function lightStrip({ k, s, ctx, variant }: Parameters<PropRecipe>[0]): void {
  const L = LIGHT_STRIP;
  const bars = variant === 0 ? [0] : [-0.09, 0.09];
  for (const cx of bars) {
    const half = variant === 0 ? 0.05 : 0.045;
    k.box(cx - half, cx + half, 0, 0.04, L.h0, L.h1, s.dark);
    k.panel(
      cx - half + 0.015,
      cx + half - 0.015,
      0.04 + DECAL_LIFT,
      L.h0 + 0.05,
      L.h1 - 0.05,
      s.glow(ctx.look.palette.lamp),
    );
  }
}

/** The cable tray run: the tray's near and far depth and its rail height. */
const TRAY = { nearD: 0.05, farD: 0.21, rail: 0.07 };

/**
 * Cable tray run: an open tray the whole edge wide, held off the wall by
 * brackets. Variant 1 adds a cover lid.
 */
function cableTray({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const T = TRAY;
  const base = RUN_BAND.h0 + 0.04;
  k.box(-1.0, 1.0, T.nearD, T.farD, base, base + 0.02, s.dark);
  k.box(-1.0, 1.0, T.nearD, T.nearD + 0.02, base, base + T.rail, s.metal);
  k.box(-1.0, 1.0, T.farD - 0.02, T.farD, base, base + T.rail, s.metal);
  for (let a = -0.9; a <= 0.9 + 1e-6; a += 0.45) {
    k.box(a - 0.02, a + 0.02, 0, T.nearD, RUN_BAND.h0, base + 0.02, s.dark);
  }
  if (variant === 1) {
    k.box(
      -1.0,
      1.0,
      T.nearD,
      T.farD,
      base + T.rail,
      base + T.rail + 0.015,
      s.dark,
    );
  }
}

/** Pipe bundle: the pipes' depth and radius. */
const PIPES = { depth: 0.1, radius: 0.035 };

/**
 * Pipe bundle run: two pipes clamped to the wall with brackets. Variant 1
 * adds a third pipe and a valve wheel at `a = 0`.
 */
function pipeBundle({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const P = PIPES;
  const rows =
    variant === 0
      ? [RUN_BAND.h0 + 0.12, RUN_BAND.h0 + 0.22]
      : [RUN_BAND.h0 + 0.08, RUN_BAND.h0 + 0.2, RUN_BAND.h0 + 0.32];
  for (const h of rows) {
    k.cylinderAlong(-1.0, 1.0, P.depth, h, P.radius, 8, s.metal);
  }
  for (const a of [-0.85, 0, 0.85]) {
    for (const h of rows) {
      k.box(
        a - 0.03,
        a + 0.03,
        P.depth - P.radius - 0.01,
        P.depth + P.radius + 0.01,
        h - P.radius - 0.01,
        h + P.radius + 0.01,
        s.dark,
      );
    }
  }
  if (variant === 1) {
    const vh = rows[2] ?? RUN_BAND.h0 + 0.32;
    const d = P.depth + P.radius + 0.03;
    k.ring(0, d, vh, 0.07, 0.012, 6, 10, s.metal, "inward");
    k.box(
      -0.02,
      0.02,
      P.depth + P.radius + 0.01,
      d + 0.02,
      vh - 0.02,
      vh + 0.02,
      s.dark,
    );
  }
}

/** The tool board: its width, depth and height band. */
const TOOL_BOARD = { half: 0.88, depth: 0.03, h0: 0.9, h1: 2.05 };

/**
 * A thin flat tool silhouette in the (a, h) plane at the given depth range:
 * `tiltedBar` gives the outline, `extrude` gives it a hair of thickness.
 */
function toolShape(
  k: Kit,
  s: Surface,
  a: number,
  h: number,
  angle: number,
  length: number,
  width: number,
  d0: number,
  d1: number,
): void {
  k.extrude(tiltedBar(a, h, angle, length, width), d0, d1, s);
}

/**
 * A hung hammer silhouette: a thin vertical handle and a short head across
 * its top, both flat boxes standing just off the board.
 */
function hammerShape(
  k: Kit,
  s: Surface,
  a: number,
  h: number,
  d0: number,
  d1: number,
): void {
  k.box(a - 0.012, a + 0.012, d0, d1, h - 0.14, h + 0.02, s);
  k.box(a - 0.09, a + 0.09, d0, d1, h + 0.02, h + 0.07, s);
}

/** A pair of crossed flat bars: an open pair of pliers. */
function pliersShape(
  k: Kit,
  s: Surface,
  a: number,
  h: number,
  d0: number,
  d1: number,
): void {
  k.extrude(tiltedBar(a, h, 0.5, 0.26, 0.03), d0, d1, s);
  k.extrude(tiltedBar(a, h, -0.5, 0.26, 0.03), d0, d1, s);
}

/**
 * Tool board: a perforated-looking board with a dark lip shelf along its
 * foot and 6 to 8 thin tool silhouettes (spanners, hammers, pliers) hung on
 * its face. Variant 1 adds a bin rail at `h = 0.95` holding 4 coloured bins
 * and thins the tools above it out.
 */
function toolBoard({ k, s, ctx, variant }: Parameters<PropRecipe>[0]): void {
  const T = TOOL_BOARD;
  k.bevelBox(-T.half, T.half, 0, T.depth, T.h0, T.h1, 0.01, s.panel);
  k.box(-T.half, T.half, T.depth, T.depth + 0.07, T.h0 - 0.05, T.h0, s.dark);
  const HOLE = 0.012;
  const step = 0.14;
  for (let hy = T.h0 + 0.08; hy <= T.h1 - 0.08; hy += step) {
    for (let a = -T.half + 0.08; a <= T.half - 0.08; a += step) {
      k.panel(
        a - HOLE,
        a + HOLE,
        T.depth + DECAL_LIFT,
        hy - HOLE,
        hy + HOLE,
        s.dark,
      );
    }
  }
  const near = T.depth + 0.005;
  const far = T.depth + 0.02;
  const row = T.h1 - 0.35;
  if (variant === 0) {
    toolShape(k, s.dark, -0.68, row, 0.15, 0.32, 0.035, near, far);
    toolShape(k, s.dark, -0.4, row, -0.15, 0.32, 0.035, near, far);
    hammerShape(k, s.dark, -0.08, row, near, far);
    pliersShape(k, s.metal, 0.2, row, near, far);
    toolShape(k, s.dark, 0.48, row, 0.2, 0.32, 0.035, near, far);
    hammerShape(k, s.dark, 0.72, row, near, far);
    toolShape(k, s.dark, -0.55, row - 0.4, 0.3, 0.32, 0.035, near, far);
    pliersShape(k, s.metal, 0.05, row - 0.4, near, far);
  } else {
    const railH = 0.95;
    k.box(
      -T.half,
      T.half,
      T.depth,
      T.depth + 0.12,
      railH,
      railH + 0.02,
      s.metal,
    );
    const binW = (2 * T.half - 0.08) / 4;
    const bins = [
      ctx.look.palette.screen,
      ctx.look.palette.door,
      ctx.look.palette.lamp,
      shade(ctx.look.palette.metal, 0.6),
    ];
    for (let i = 0; i < 4; i++) {
      const a0 = -T.half + 0.04 + i * binW;
      const tint = bins[i] ?? ctx.look.palette.metal;
      k.bevelBox(
        a0,
        a0 + binW - 0.02,
        T.depth + 0.02,
        T.depth + 0.12,
        railH + 0.02,
        railH + 0.16,
        0.01,
        s.tinted(tint),
      );
    }
    toolShape(k, s.dark, -0.5, row, 0.15, 0.32, 0.035, near, far);
    hammerShape(k, s.dark, 0.0, row, near, far);
    pliersShape(k, s.metal, 0.5, row, near, far);
  }
}

/**
 * The conduit cabinet: its depth, the horizontal conduit's height, variant
 * 0's one cabinet from `a0` to `a1` along the wall, and variant 1's two
 * cabinets, which run from `-half` to `-gap` and from `gap` to `half`.
 */
const CONDUIT = {
  depth: 0.28,
  runH: 2.15,
  one: { a0: -0.55, a1: 0.25 },
  two: { half: 0.88, gap: 0.06 },
};

/**
 * The wall props that stand on the floor against their wall, per variant:
 * the body's extent along the wall (`a0` to `a1`) and its depth out from
 * it, as the recipes above build them. The contact shadows read it.
 */
export const WALL_STANDING: Partial<
  Record<WallPropKind, readonly { a0: number; a1: number; depth: number }[]>
> = {
  "locker-bank": [0, 1].map(() => ({
    a0: -LOCKER.half,
    a1: LOCKER.half,
    depth: LOCKER.depth,
  })),
  "conduit-cabinet": [
    { ...CONDUIT.one, depth: CONDUIT.depth },
    { a0: -CONDUIT.two.half, a1: CONDUIT.two.half, depth: CONDUIT.depth },
  ],
};

/**
 * Conduit cabinet: variant 0 one cabinet with a door seam, a handle and two
 * conduits climbing to a horizontal run across the whole reach with a
 * junction box at each end; variant 1 two narrower cabinets side by side,
 * each feeding the same run.
 */
function conduitCabinet({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const C = CONDUIT;
  const d = C.depth / 2;
  if (variant === 0) {
    const { a0, a1 } = C.one;
    k.bevelBox(a0, a1, 0, C.depth, 0, 1.9, 0.015, s.body);
    const seam = -0.15;
    k.box(
      seam - 0.004,
      seam + 0.004,
      C.depth - 0.01,
      C.depth + 0.002,
      0.05,
      1.85,
      s.dark,
    );
    k.box(
      seam - 0.09,
      seam - 0.03,
      C.depth - 0.005,
      C.depth + 0.018,
      0.9,
      0.98,
      s.metal,
    );
    // A trim along the cabinet's top, drawn only in a look with the
    // prop's own accent.
    const trim = s.own(s.body);
    if (trim !== s.body)
      k.box(a0, a1, C.depth - 0.01, C.depth + 0.004, 1.78, 1.84, trim);
    for (const a of [-0.4, 0.1]) k.cylinder(a, d, 1.9, 2.2, 0.03, 8, s.metal);
    k.cylinderAlong(-0.88, 0.88, d, C.runH, 0.035, 8, s.metal);
    for (const a of [-0.85, 0.85]) {
      k.box(
        a - 0.04,
        a + 0.04,
        d - 0.05,
        d + 0.05,
        C.runH - 0.06,
        C.runH + 0.06,
        s.dark,
      );
    }
  } else {
    const { half, gap } = C.two;
    for (const [a0, a1] of [
      [-half, -gap],
      [gap, half],
    ] as const) {
      k.bevelBox(a0, a1, 0, C.depth, 0, 1.7, 0.015, s.body);
      const mid = (a0 + a1) / 2;
      // A trim along the left cabinet's top, drawn only in a look with the
      // prop's own accent.
      const trim = s.own(s.body);
      if (a0 < 0 && trim !== s.body)
        k.box(a0, a1, C.depth - 0.01, C.depth + 0.004, 1.58, 1.64, trim);
      k.box(
        mid - 0.004,
        mid + 0.004,
        C.depth - 0.01,
        C.depth + 0.002,
        0.05,
        1.65,
        s.dark,
      );
      k.cylinder(mid, d, 1.7, C.runH, 0.03, 8, s.metal);
    }
    k.cylinderAlong(-0.88, 0.88, d, C.runH, 0.035, 8, s.metal);
  }
}

/** The pipe riser: the pipes' depth and its mounting strap's height. */
const RISER = { depth: 0.12, strapH: 2.05 };

/**
 * Pipe riser: variant 0 three vertical pipes on wall brackets with flanges
 * at `h = 0.4` and `1.8` and a hand-wheel valve on the middle one at
 * `h = 1.2`; variant 1 two thicker pipes with a crossover pipe between them
 * at `h = 0.9` and a glowing gauge disc on the left pipe at `h = 1.5`. A
 * mounting strap ties the pipes to the wall at `RISER.strapH`, wide enough
 * to reach `WIDE_REACH` on both sides (D4), grounded to the wall by a small
 * bracket at each end.
 */
function pipeRiser({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const R = RISER;
  k.box(
    -0.88,
    0.88,
    R.depth - 0.02,
    R.depth + 0.02,
    R.strapH,
    R.strapH + 0.04,
    s.dark,
  );
  for (const a of [-0.85, 0.85]) {
    k.box(
      a - 0.03,
      a + 0.03,
      0,
      R.depth - 0.02,
      R.strapH,
      R.strapH + 0.04,
      s.dark,
    );
  }
  if (variant === 0) {
    for (const a of [-0.7, 0, 0.7]) {
      k.cylinder(a, R.depth, 0, 2.2, 0.05, 10, s.metal);
      for (const h of [0.4, 1.8]) {
        k.cylinder(a, R.depth, h - 0.02, h + 0.02, 0.08, 10, s.dark);
      }
      k.box(a - 0.02, a + 0.02, 0, R.depth + 0.05, 0.88, 0.92, s.dark);
    }
    const pipeFront = R.depth + 0.05;
    const ringD = pipeFront + 0.03;
    k.ring(0, ringD, 1.2, 0.09, 0.012, 6, 10, s.metal, "inward");
    k.box(-0.02, 0.02, pipeFront + 0.01, ringD + 0.02, 1.18, 1.22, s.dark);
    k.cylinder(0, R.depth, 1.15, 1.25, 0.065, 8, s.dark);
  } else {
    for (const a of [-0.6, 0.6])
      k.cylinder(a, R.depth, 0, 2.2, 0.07, 10, s.metal);
    k.cylinderAlong(-0.6, 0.6, R.depth, 0.9, 0.045, 8, s.metal);
    const pipeFront = R.depth + 0.07;
    const gaugeFace = pipeFront + 0.04;
    k.extrude(discOutline(-0.6, 1.5, 0.06), pipeFront, gaugeFace, s.dark);
    k.panel(-0.64, -0.56, gaugeFace + DECAL_LIFT, 1.46, 1.54, s.glow(GREEN));
  }
}

/** The stowage net: its posts' reach, the net's depth and the posts' band. */
const NET = { half: 0.86, netD: 0.24, h0: 0.1, h1: 2.1 };

/**
 * Stowage net: two posts a whole edge apart with a diamond mesh of thin
 * crossed bars strung between them, and 3 or 4 stowed items behind it.
 * Variant 1 adds two horizontal straps and bulkier stowed items.
 */
function stowageNet({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const N = NET;
  for (const a of [-N.half, N.half]) {
    k.cylinder(a, N.netD - 0.02, N.h0, N.h1, 0.025, 8, s.metal);
    for (const h of [N.h0, N.h1]) {
      k.box(a - 0.03, a + 0.03, 0, N.netD - 0.01, h - 0.02, h + 0.02, s.dark);
    }
  }
  k.box(-N.half, N.half, 0.02, N.netD - 0.02, N.h0, N.h0 + 0.05, s.dark);
  const rows = 4;
  const bandH = (N.h1 - N.h0) / rows;
  const diag = Math.atan2(bandH, 2 * N.half);
  const length = Math.hypot(2 * N.half, bandH);
  for (let i = 0; i < rows; i++) {
    const cy = N.h0 + bandH * (i + 0.5);
    toolShape(
      k,
      s.dark,
      0,
      cy,
      diag,
      length,
      0.012,
      N.netD - 0.006,
      N.netD + 0.006,
    );
    toolShape(
      k,
      s.dark,
      0,
      cy,
      -diag,
      length,
      0.012,
      N.netD - 0.006,
      N.netD + 0.006,
    );
  }
  if (variant === 1) {
    for (const h of [0.7, 1.5]) {
      k.box(
        -N.half,
        N.half,
        N.netD - 0.015,
        N.netD + 0.015,
        h - 0.02,
        h + 0.02,
        s.metal,
      );
    }
  }
  const items =
    variant === 0
      ? [
          { a0: -0.62, a1: -0.28, h0: 0.15, h1: 0.75 },
          { a0: -0.12, a1: 0.22, h0: 0.15, h1: 0.55 },
          { a0: 0.32, a1: 0.6, h0: 0.15, h1: 0.9 },
        ]
      : [
          { a0: -0.66, a1: -0.2, h0: 0.15, h1: 1.05 },
          { a0: -0.1, a1: 0.34, h0: 0.15, h1: 0.85 },
          { a0: 0.4, a1: 0.7, h0: 0.15, h1: 1.15 },
          { a0: -0.3, a1: -0.02, h0: 1.05, h1: 1.35 },
        ];
  for (const it of items) {
    k.bevelBox(it.a0, it.a1, 0.02, 0.22, it.h0, it.h1, 0.015, s.body);
  }
}

/** The recipe of every wall prop kind, runs included. */
export const WALL_RECIPES = {
  "locker-bank": lockerBank,
  extinguisher,
  "first-aid": firstAid,
  intercom,
  "keycard-reader": keycardReader,
  "vent-grille": ventGrille,
  "sign-plate": signPlate,
  "breaker-box": breakerBox,
  "wall-monitor": wallMonitor,
  "padded-panel": paddedPanel,
  "light-strip": lightStrip,
  "cable-tray": cableTray,
  "pipe-bundle": pipeBundle,
  "tool-board": toolBoard,
  "conduit-cabinet": conduitCabinet,
  "stowage-net": stowageNet,
  "pipe-riser": pipeRiser,
} satisfies Record<Exclude<WallPropKind, RarePropKind>, PropRecipe>;
