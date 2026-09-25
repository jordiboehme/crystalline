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
 */

import type { CeilingPropKind } from "../../../world/types";
import type { Rgb } from "../../looks";
import { HEADROOM, tiltedBar } from "../common";
import type { Kit } from "../../kit";
import {
  CEILING_DROP,
  CEILING_OUT,
  CEILING_SETBACK,
  RUN_REACH,
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

/** The recipe of every ceiling prop kind, runs included. */
export const CEILING_RECIPES = {
  duct,
  "ceiling-tray": ceilingTray,
  "cable-loop": cableLoop,
  beacon,
  "loose-cable": looseCable,
} satisfies Record<CeilingPropKind, PropRecipe>;
