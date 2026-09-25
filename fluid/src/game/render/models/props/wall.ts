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
 */

import { FLAG } from "../../geometry";
import { DECAL_LIFT, frameAt } from "../../kit";
import { LAYER } from "../../layers";
import type { Rgb } from "../../looks";
import { PICTOGRAM, SIGN_PICTOGRAMS } from "../../text";
import type { WallPropKind } from "../../../world/types";
import { yawed } from "../common";
import { RUN_BAND, type PropRecipe } from "./common";

/** A warm hazard red: the extinguisher's cylinder and the first-aid cross. */
const RED: Rgb = [0.74, 0.09, 0.07];

/** A status green: the keycard reader's ready glow. */
const GREEN: Rgb = [0.22, 0.92, 0.34];

/**
 * A thin rectangle in the `(a, h)` plane, centred at `(cx, cy)`, `length`
 * long at `angle` radians from the `a` axis and `width` wide: an outline
 * for `k.extrude`, used for a fan blade, a slanted louvre or any other
 * flat bar that does not sit flush along `a` or `h` alone.
 */
function tiltedBar(
  cx: number,
  cy: number,
  angle: number,
  length: number,
  width: number,
): [number, number][] {
  const dx = Math.cos(angle) * (length / 2);
  const dy = Math.sin(angle) * (length / 2);
  const nx = -Math.sin(angle) * (width / 2);
  const ny = Math.cos(angle) * (width / 2);
  return [
    [cx - dx + nx, cy - dy + ny],
    [cx + dx + nx, cy + dy + ny],
    [cx + dx - nx, cy + dy - ny],
    [cx - dx - nx, cy - dy - ny],
  ];
}

/** The locker bank: its overall box and how tall its doors run. */
const LOCKER = { half: 0.85, depth: 0.26, h1: 2.0 };

/**
 * Locker bank: a cabinet of narrow doors, each with a vent slit and a
 * handle. Variant 0 has three doors, variant 1 four narrower ones.
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
        s.dark,
      );
    }
    k.box(
      a1 - 0.06,
      a1 - 0.03,
      L.depth - 0.005,
      L.depth + 0.03,
      L.h1 / 2 - 0.09,
      L.h1 / 2 + 0.09,
      s.metal,
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
function signPlate({ k, variant }: Parameters<PropRecipe>[0]): void {
  const P = SIGN;
  const key = SIGN_PICTOGRAMS[variant];
  if (key === undefined) {
    throw new Error(`sign-plate: no pictogram for variant ${String(variant)}`);
  }
  const rect = PICTOGRAM[key];
  k.bevelBox(-P.half, P.half, 0, P.depth, P.h0, P.h1, 0.01, {
    layer: LAYER.metal,
    tint: [0.2, 0.2, 0.22],
    flag: FLAG.lit,
  });
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
      s.dark,
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
      s.dark,
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

/** The padded panel: its overall size and cushion depth. */
const PADDED = { half: 0.4, h0: 0.6, h1: 2.0, depth: 0.06 };

/**
 * Padded panel: a quilted wall of bevelled cushions, 2 by 3 in variant 0
 * and 3 by 4 in variant 1.
 */
function paddedPanel({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const P = PADDED;
  const cols = variant === 0 ? 2 : 3;
  const rows = variant === 0 ? 3 : 4;
  const gap = 0.01;
  const cw = (2 * P.half - gap * (cols - 1)) / cols;
  const rh = (P.h1 - P.h0 - gap * (rows - 1)) / rows;
  const bevel = Math.min(0.015, cw * 0.2, rh * 0.2);
  const quilt = s.tinted([0.55, 0.5, 0.46]);
  for (let c = 0; c < cols; c++) {
    for (let r = 0; r < rows; r++) {
      const a0 = -P.half + c * (cw + gap);
      const h0 = P.h0 + r * (rh + gap);
      k.bevelBox(a0, a0 + cw, 0, P.depth, h0, h0 + rh, bevel, quilt);
    }
  }
}

/** The light strip housing: its height band. */
const LIGHT_STRIP = { h0: 0.3, h1: 2.3 };

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
} satisfies Record<WallPropKind, PropRecipe>;
