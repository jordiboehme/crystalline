/**
 * The ceiling props' recipes: everything that hangs from the ceiling, out
 * of the player's way, and the two ceiling runs.
 *
 * A ceiling prop hangs from its anchor at the room's ceiling, so its `h`
 * runs from `-CEILING_DROP` to `-HEADROOM` (the part nearest the ceiling,
 * a hanger strap or a mounting plate, stops exactly at `-HEADROOM`); it
 * stands `CEILING_SETBACK` to `CEILING_OUT` out from its wall and within
 * `WALL_REACH` of its anchor along it, or `RUN_REACH` for a run segment
 * (`duct`, `ceiling-tray`). Every recipe is built at turn 0 in the kit's
 * local `(a, d, h)` terms, exactly like a wall prop, and never sees the
 * room.
 *
 * A span (`span-duct`, `span-tray`) hangs over the hall's interior rather
 * than along a wall, so it reads `(a, d, h)` differently: `d` is centred on
 * its line (`|d| <= SPAN_HALF`, not `D_MID`) and `a` reaches `SPAN_REACH`
 * either side, the whole segment, so neighbouring segments meet exactly as
 * a run's do. `h` keeps the same band as every ceiling prop.
 */

import type { CeilingPropKind } from "../../../world/types";
import type { Rgb } from "../../looks";
import { HEADROOM, tiltedBar, type Surfaces } from "../common";
import type { Kit } from "../../kit";
import {
  CEILING_DROP,
  CEILING_OUT,
  CEILING_SETBACK,
  RUN_REACH,
  SPAN_HALF,
  SPAN_REACH,
  type PropRecipe,
} from "./common";

/** A warm hazard red: the beacon's lens. */
const RED: Rgb = [0.8, 0.08, 0.06];

/**
 * How far out from the wall a ceiling prop's body sits, in metres: the
 * middle of `[CEILING_SETBACK, CEILING_OUT]`, with equal room to spare on
 * both sides for a hanger's own width or a run's rails.
 */
const D_MID = (CEILING_SETBACK + CEILING_OUT) / 2;

/**
 * Evenly spaced positions from `start` to `end`, `step` apart, both ends
 * included: an integer count times `step`, never a repeated float
 * addition, which drifts and can silently drop the last position (as it
 * did for the span tray's rungs, leaving a bare end).
 */
function evenlySpaced(start: number, end: number, step: number): number[] {
  const count = Math.round((end - start) / step) + 1;
  return Array.from({ length: count }, (_, i) => start + i * step);
}

/** A hanger strap from its host's nearest-to-ceiling surface up to exactly `-HEADROOM`. */
function hanger(
  k: Kit,
  s: Parameters<PropRecipe>[0]["s"],
  a: number,
  d: number,
  hostNear: number,
  half = 0.02,
): void {
  k.box(a - half, a + half, d - half, d + half, hostNear, -HEADROOM, s.metal);
}

/** The duct's radius (round variant) and half-size (square variant), and its centre height. */
const DUCT = {
  radius: 0.15,
  half: 0.14,
  h: -(CEILING_DROP + HEADROOM) / 2 - 0.1,
};

/**
 * Duct run: variant 0 a round duct the whole edge long with hanger straps
 * every third of the way; variant 1 a square duct with seam lines instead.
 */
function duct({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const D = DUCT;
  const d = D_MID;
  let near: number;
  if (variant === 0) {
    k.cylinderAlong(-RUN_REACH, RUN_REACH, d, D.h, D.radius, 10, s.metal);
    near = D.h + D.radius;
  } else {
    k.box(
      -RUN_REACH,
      RUN_REACH,
      d - D.half,
      d + D.half,
      D.h - D.half,
      D.h + D.half,
      s.metal,
    );
    for (let a = -RUN_REACH + 0.3; a <= RUN_REACH - 0.15; a += 0.4) {
      k.box(
        a - 0.006,
        a + 0.006,
        d - D.half - 0.004,
        d + D.half + 0.004,
        D.h - D.half,
        D.h + D.half,
        s.dark,
      );
    }
    near = D.h + D.half;
  }
  for (const a of [-0.7, 0, 0.7]) hanger(k, s, a, d, near, 0.02);
}

/** The tray's half-width across the run and its rail height. */
const TRAY = {
  half: 0.16,
  rail: 0.05,
  h: -(CEILING_DROP + HEADROOM) / 2 - 0.15,
};

/**
 * Ceiling tray run: variant 0 an open tray the whole edge wide with cables
 * resting in it; variant 1 a mesh tray (cross ribs instead of a solid base).
 */
function ceilingTray({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const T = TRAY;
  const d0 = D_MID - T.half;
  const d1 = D_MID + T.half;
  let near: number;
  if (variant === 0) {
    k.box(-RUN_REACH, RUN_REACH, d0, d1, T.h, T.h + 0.02, s.dark);
    for (let i = 0; i < 4; i++) {
      const cd = d0 + 0.05 + i * ((d1 - d0 - 0.1) / 3);
      k.cylinderAlong(
        -RUN_REACH + 0.05,
        RUN_REACH - 0.05,
        cd,
        T.h + 0.05,
        0.018,
        6,
        s.metal,
      );
    }
    near = T.h + 0.02;
  } else {
    for (let a = -RUN_REACH + 0.1; a <= RUN_REACH - 0.05; a += 0.2) {
      k.box(a - 0.008, a + 0.008, d0, d1, T.h, T.h + 0.015, s.dark);
    }
    k.box(
      -RUN_REACH,
      RUN_REACH,
      d0 - 0.01,
      d0 + 0.008,
      T.h,
      T.h + T.rail,
      s.metal,
    );
    k.box(
      -RUN_REACH,
      RUN_REACH,
      d1 - 0.008,
      d1 + 0.01,
      T.h,
      T.h + T.rail,
      s.metal,
    );
    near = T.h + T.rail;
  }
  for (const a of [-0.7, 0, 0.7]) hanger(k, s, a, D_MID, near, 0.02);
}

/**
 * One sagging chain between two hangers, centred at `aCenter` and spanning
 * `2 * half` along the wall: two straps down to the loop's attach height,
 * then a handful of short tilted links (`extrude` on a bar in the `(a, h)`
 * plane, the natural plane for a droop that runs along the wall) stepping
 * down to a low point and back up, tracing a sine sag between the anchors.
 */
function sagLoop(
  k: Kit,
  s: Parameters<PropRecipe>[0]["s"],
  aCenter: number,
  half: number,
): void {
  const d = D_MID;
  const hTop = -0.2;
  const hLow = -0.5;
  const segs = 6;
  const pts: [number, number][] = Array.from({ length: segs + 1 }, (_, i) => {
    const t = i / segs;
    const a = aCenter - half + 2 * half * t;
    const h = hTop - (hTop - hLow) * Math.sin(Math.PI * t);
    return [a, h];
  });
  for (const a of [aCenter - half, aCenter + half]) {
    k.box(a - 0.02, a + 0.02, d - 0.02, d + 0.02, hTop, -HEADROOM, s.metal);
  }
  const thick = 0.035;
  for (let i = 0; i < segs; i++) {
    const [a0, h0] = pts[i] ?? [aCenter, hTop];
    const [a1, h1] = pts[i + 1] ?? [aCenter, hTop];
    const angle = Math.atan2(h1 - h0, a1 - a0);
    const length = Math.hypot(a1 - a0, h1 - h0);
    const bar = tiltedBar((a0 + a1) / 2, (h0 + h1) / 2, angle, length, thick);
    k.extrude(bar, d - 0.018, d + 0.018, s.dark);
  }
}

/**
 * Cable loop: variant 0 one sagging loop between two hangers, variant 1 two
 * loops side by side, sharing the run's width.
 */
function cableLoop({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  if (variant === 0) {
    sagLoop(k, s, 0, 0.55);
    return;
  }
  sagLoop(k, s, -0.4, 0.35);
  sagLoop(k, s, 0.4, 0.35);
}

/**
 * Beacon: a housing flush with the ceiling and a lathed emissive lens
 * hanging just below it, touching its base. Variant 1 adds a wire cage
 * around the lens.
 */
function beacon({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const d = D_MID;
  const h0 = -0.25;
  k.bevelBox(-0.11, 0.11, d - 0.09, d + 0.09, h0, -HEADROOM, 0.012, s.dark);
  k.lathe(
    0,
    d,
    [
      [0, h0],
      [0.08, h0 - 0.02],
      [0.09, h0 - 0.11],
      [0.06, h0 - 0.16],
      [0, h0 - 0.18],
    ],
    12,
    s.glow(RED),
  );
  if (variant === 1) {
    for (let i = 0; i < 4; i++) {
      const angle = (Math.PI / 2) * i;
      const a = Math.sin(angle) * 0.1;
      const dd = d + Math.cos(angle) * 0.1;
      k.cylinderAlong(a - 0.006, a + 0.006, dd, h0 - 0.09, 0.006, 4, s.metal);
    }
    k.ring(0, d, h0 - 0.14, 0.1, 0.008, 6, 10, s.metal, "up");
  }
}

/**
 * Loose cable: variant 0 one thin cable hanging from the ceiling to
 * `-CEILING_DROP` with a small frayed burst at its tip; variant 1 two
 * cables side by side.
 */
function looseCable({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const positions = variant === 0 ? [0] : [-0.3, 0.3];
  const d = D_MID;
  for (const a of positions) {
    const tip = -CEILING_DROP + 0.05;
    k.cylinder(a, d, tip, -HEADROOM, 0.014, 6, s.dark);
    for (let i = 0; i < 3; i++) {
      const angle = ((Math.PI * 2) / 3) * i;
      const fa = a + Math.sin(angle) * 0.03;
      const fd = d + Math.cos(angle) * 0.03;
      k.cylinder(fa, fd, tip - 0.05, tip, 0.004, 4, s.dark);
    }
  }
}

/** The round duct's height, radius and flange half-width along `a`. */
const SPAN_DUCT_ROUND = { h: -0.45, radius: 0.2, flange: 0.03 };
/** The square duct's height and half-size across it, kept inside D9's `SPAN_HALF`. */
const SPAN_DUCT_SQUARE = { h: -0.45, halfW: SPAN_HALF - 0.05, halfH: 0.15 };

/**
 * Span duct: variant 0 a round duct the whole segment long with a flange
 * collar at its anchor; variant 1 a square duct with seam lines every
 * 0.5 m. Both carry hanger straps at `a = +-1.0`, and both stay within
 * `SPAN_HALF` across their line, well inside a wall duct's own reach.
 */
function spanDuct({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  if (variant === 0) {
    const D = SPAN_DUCT_ROUND;
    k.cylinderAlong(-SPAN_REACH, SPAN_REACH, 0, D.h, D.radius, 10, s.metal);
    k.cylinderAlong(-D.flange, D.flange, 0, D.h, D.radius + 0.03, 10, s.metal);
    for (const a of [-1.0, 1.0]) hanger(k, s, a, 0, D.h + D.radius, 0.02);
  } else {
    const D = SPAN_DUCT_SQUARE;
    k.box(
      -SPAN_REACH,
      SPAN_REACH,
      -D.halfW,
      D.halfW,
      D.h - D.halfH,
      D.h + D.halfH,
      s.metal,
    );
    for (const a of evenlySpaced(-SPAN_REACH + 0.3, SPAN_REACH - 0.15, 0.5)) {
      k.box(
        a - 0.006,
        a + 0.006,
        -D.halfW - 0.004,
        D.halfW + 0.004,
        D.h - D.halfH,
        D.h + D.halfH,
        s.dark,
      );
    }
    for (const a of [-1.0, 1.0]) hanger(k, s, a, 0, D.h + D.halfH, 0.02);
  }
}

/** The ladder tray's half-width across the line (D9's `SPAN_HALF` less its rails). */
const SPAN_TRAY = { halfW: SPAN_HALF - 0.1 };

/** One ladder-tray tier at height `h`: two side rails, rungs and 3 resting cables. */
function trayTier(k: Kit, s: Surfaces, h: number): void {
  const half = SPAN_TRAY.halfW;
  for (const d of [-half, half]) {
    k.box(
      -SPAN_REACH,
      SPAN_REACH,
      d - 0.01,
      d + 0.01,
      h - 0.015,
      h + 0.015,
      s.metal,
    );
  }
  for (const a of evenlySpaced(-SPAN_REACH + 0.2, SPAN_REACH - 0.2, 0.4)) {
    k.box(a - 0.008, a + 0.008, -half, half, h - 0.01, h + 0.01, s.dark);
  }
  for (const d of [-0.15, 0, 0.15]) {
    k.cylinderAlong(
      -SPAN_REACH + 0.05,
      SPAN_REACH - 0.05,
      d,
      h + 0.03,
      0.02,
      6,
      s.dark,
    );
  }
}

/**
 * Span tray: variant 0 one ladder tray at `h = -0.5` with four threaded-rod
 * hangers (reusing `hanger`) at `a = +-1.5`, one on each rail so they run
 * up alongside the tray rather than through the cables in its middle;
 * variant 1 two tiers, at `h = -0.35` and `h = -0.6`, joined by the same
 * four hangers, which span from the lower tier's rail top past the upper
 * tier's own rails to `-HEADROOM`.
 */
function spanTray({ k, s, variant }: Parameters<PropRecipe>[0]): void {
  const half = SPAN_TRAY.halfW;
  const rods = (hostNear: number) => {
    for (const a of [-1.5, 1.5])
      for (const d of [-half, half]) hanger(k, s, a, d, hostNear, 0.012);
  };
  if (variant === 0) {
    const h = -0.5;
    trayTier(k, s, h);
    rods(h + 0.015);
  } else {
    const upperH = -0.35;
    const lowerH = -0.6;
    trayTier(k, s, upperH);
    trayTier(k, s, lowerH);
    rods(lowerH + 0.015);
  }
}

/** The recipe of every ceiling prop kind, runs included. */
export const CEILING_RECIPES = {
  duct,
  "ceiling-tray": ceilingTray,
  "cable-loop": cableLoop,
  beacon,
  "loose-cable": looseCable,
  "span-duct": spanDuct,
  "span-tray": spanTray,
} satisfies Record<CeilingPropKind, PropRecipe>;
