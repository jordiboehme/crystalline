/**
 * The airlock's fittings (M3 C24): the outer hatch, the two amber beacons
 * beside it, the iris light in the ceiling and the suit lockers along the
 * side walls. Like the console room's, each is built once at the origin in
 * `frameAt([0, 0, 0], 0)`, `a` across, `d` out from the wall (or front to
 * back for the centred iris light) and `h` up, in fixed tints whatever
 * the look (the family's rule, 2.6e C4): the airlock's shell is drawn in
 * the same fixed white as the console room's.
 *
 * - The **outer hatch** is a backed piece: a round collar standing out of
 *   the wall, cut into `HATCH.segments` sectors alternately hazard yellow
 *   and black; inside it the sealed door, a pale disc set back behind the
 *   collar with a raised plate on it; eight locking dogs bridging the
 *   door's rim and the collar; a handwheel of four spokes on a hub at the
 *   door's centre; and the `AIRLOCK` stencil (`MARKS.airlockWord`) over
 *   the wheel, set through `textRows` and laid by `pixelPanel`. It never
 *   opens and nothing uses it: it is no fixture, only the airlock's outer
 *   wall.
 * - A **beacon** is a flush piece: a small wall plate and a bracket
 *   holding a round base, a steady amber ring on the base (`s.signal`, so
 *   the beacon is never fully dark) and an amber dome that blinks on the
 *   `swap` bank. Variant 0 blinks in group 0, variant 1 in group 4, so the
 *   two beside the hatch take turns every `SWAP_TICS`; the dome's tint is
 *   above 1, so even its low half reads lit.
 * - The **iris light** is an overhead piece, centred on its point and hung
 *   from the ceiling at the catalogue's `top`: a dark round mount, a lit
 *   core under it, eight iris blades round the core, overlapping like a
 *   half-open aperture, and eight hazard-yellow locking dogs round the
 *   mount's rim, so it reads as a pressure hatch in the ceiling that
 *   lets the light through.
 * - A **suit locker** is a backed piece against a side wall: a slate grey
 *   cabinet on a black plinth with a paler door, a dim blue window, a
 *   handle, three vent slots, a hazard-yellow tag and a small green
 *   status light.
 *
 * Only the outer hatch sets text: its one stencil.
 */

import type { InteriorKind } from "../../../world/types";
import { FLAG, blinkFlag, type Surface } from "../../geometry";
import { frameAt } from "../../kit";
import { LAYER } from "../../layers";
import type { Rgb } from "../../looks";
import { discOutline, yawed } from "../common";
import { MARK_PROUD, fit, pixelPanel, textRows } from "../heroes/pixels";
import { MARKS } from "../marks";
import { interiorHalf, type InteriorRecipe } from "./common";

/** The hazard collar's two tones, and the airlock's fittings' fixed tints. */
export const AIRLOCK_TINT = {
  yellow: [0.95, 0.72, 0.1],
  black: [0.07, 0.07, 0.07],
  door: [0.8, 0.82, 0.84],
  plate: [0.88, 0.9, 0.92],
  steel: [0.46, 0.48, 0.5],
  dark: [0.16, 0.17, 0.18],
  ink: [0.1, 0.1, 0.1],
  locker: [0.34, 0.38, 0.44],
  lockerDoor: [0.56, 0.6, 0.66],
  window: [0.16, 0.26, 0.36],
  green: [0.35, 1, 0.45],
  amberBase: [1.0, 0.5, 0.08],
  amber: [1.6, 0.85, 0.15],
  core: [1.1, 1.08, 1.0],
} as const satisfies Record<string, Rgb>;

/** A fixed tint on the metal layer, lit by the room. */
const metal = (tint: Rgb): Surface => ({
  layer: LAYER.metal,
  tint,
  flag: FLAG.lit,
});

/** A fixed tint on the panel layer, lit by the room. */
const paint = (tint: Rgb): Surface => ({
  layer: LAYER.panel,
  tint,
  flag: FLAG.lit,
});

/**
 * The outer hatch's measures: its centre's height, the collar's outer
 * and inner radius and depth, the door's radius and depth, the raised
 * plate's radius and face, the dogs' reach and depth, the wheel's radius,
 * depth and tube, and how many sectors the collar is cut into.
 */
export const HATCH = {
  centre: 1.95,
  outer: 1.95,
  inner: 1.5,
  collarD: 0.22,
  doorD: 0.12,
  plateR: 1.35,
  plateD: 0.16,
  dogD: 0.3,
  wheelR: 0.5,
  wheelD: 0.34,
  tube: 0.045,
  segments: 24,
} as const;

/** A rectangle `len` long and `half * 2` wide, from `r0` out along angle `t` round `(a, h)`. */
function spoke(
  a: number,
  h: number,
  t: number,
  r0: number,
  r1: number,
  half: number,
): [number, number][] {
  const [c, s] = [Math.cos(t), Math.sin(t)];
  const at = (r: number, w: number): [number, number] => [
    a + r * c - w * s,
    h + r * s + w * c,
  ];
  return [at(r0, -half), at(r1, -half), at(r1, half), at(r0, half)];
}

const outerHatch: InteriorRecipe = ({ k }) => {
  const H = HATCH.centre;
  // The collar: sectors alternately yellow and black.
  for (let i = 0; i < HATCH.segments; i++) {
    const t0 = (2 * Math.PI * i) / HATCH.segments;
    const t1 = (2 * Math.PI * (i + 1)) / HATCH.segments;
    const at = (r: number, t: number): [number, number] => [
      r * Math.cos(t),
      H + r * Math.sin(t),
    ];
    k.extrude(
      [
        at(HATCH.inner, t0),
        at(HATCH.outer, t0),
        at(HATCH.outer, t1),
        at(HATCH.inner, t1),
      ],
      0,
      HATCH.collarD,
      metal(i % 2 === 0 ? AIRLOCK_TINT.yellow : AIRLOCK_TINT.black),
    );
  }
  // The sealed door, set back in the collar, and its raised plate.
  k.extrude(
    discOutline(0, H, HATCH.inner, 32),
    0,
    HATCH.doorD,
    metal(AIRLOCK_TINT.door),
  );
  k.extrude(
    discOutline(0, H, HATCH.plateR, 32),
    HATCH.doorD,
    HATCH.plateD,
    metal(AIRLOCK_TINT.plate),
  );
  // Eight locking dogs across the door's rim and the collar's inner edge.
  for (let i = 0; i < 8; i++) {
    const t = (2 * Math.PI * (i + 0.5)) / 8;
    k.extrude(
      spoke(0, H, t, HATCH.inner - 0.16, HATCH.inner + 0.14, 0.09),
      HATCH.doorD,
      HATCH.dogD,
      metal(AIRLOCK_TINT.steel),
    );
  }
  // The handwheel: a hub, four spokes and the rim.
  const steel = metal(AIRLOCK_TINT.steel);
  k.extrude(
    discOutline(0, H, 0.12, 12),
    HATCH.plateD,
    HATCH.wheelD + 0.05,
    steel,
  );
  for (let i = 0; i < 4; i++)
    k.extrude(
      spoke(0, H, (Math.PI * i) / 2 + Math.PI / 4, 0.1, HATCH.wheelR, 0.025),
      HATCH.wheelD - 0.03,
      HATCH.wheelD + 0.03,
      steel,
    );
  k.ring(0, HATCH.wheelD, H, HATCH.wheelR, HATCH.tube, 6, 20, steel, "inward");
  // The stencil over the wheel, printed on the plate.
  const rows = textRows(MARKS.airlockWord);
  const { px, left, top } = fit(rows, -0.7, 0.7, H + 0.62, H + 0.92);
  const ink = paint(AIRLOCK_TINT.ink);
  pixelPanel(k, rows, left, top, px, HATCH.plateD + MARK_PROUD, (ch) =>
    ch === "#" ? ink : null,
  );
};

/** A beacon's measures: its lamp's centre out from the wall, and heights. */
const BEACON = { d: 0.18, base: 2.56, ring: 2.64, dome: 2.67, top: 2.88 };

const beacon: InteriorRecipe = ({ k, variant }) => {
  const dark = metal(AIRLOCK_TINT.dark);
  k.box(-0.1, 0.1, 0, 0.03, 2.3, 2.62, dark);
  k.box(-0.04, 0.04, 0.03, 0.24, 2.5, BEACON.base, dark);
  k.cylinder(0, BEACON.d, BEACON.base, BEACON.ring, 0.1, 12, dark);
  k.cylinder(0, BEACON.d, BEACON.ring, BEACON.dome, 0.09, 12, {
    layer: LAYER.panel,
    tint: AIRLOCK_TINT.amberBase,
    flag: FLAG.signal,
  });
  k.lathe(
    0,
    BEACON.d,
    [
      [0, BEACON.dome],
      [0.08, BEACON.dome],
      [0.08, 2.78],
      [0.05, 2.86],
      [0, BEACON.top],
    ],
    12,
    {
      layer: LAYER.panel,
      tint: AIRLOCK_TINT.amber,
      flag: blinkFlag(variant === 0 ? 0 : 4),
    },
  );
};

const irisLight: InteriorRecipe = ({ k, kitAt, kind }) => {
  const top = interiorHalf(kind).top;
  const mount = top - 0.08;
  k.cylinder(0, 0, mount, top, 2.15, 32, metal(AIRLOCK_TINT.dark));
  k.cylinder(0, 0, top - 0.24, mount, 0.62, 24, {
    layer: LAYER.panel,
    tint: AIRLOCK_TINT.core,
    flag: FLAG.signal,
  });
  // Every fitting is built at the origin facing north (`buildInterior`).
  const f = frameAt([0, 0, 0], 0);
  for (let i = 0; i < 8; i++) {
    const t = (2 * Math.PI * i) / 8;
    const blade = kitAt(yawed(f, 0, 0, t));
    blade.box(0.55, 1.85, 0, 0.75, top - 0.2, mount, metal(AIRLOCK_TINT.steel));
    const dog = kitAt(yawed(f, 0, 0, t + Math.PI / 8));
    dog.box(
      1.9,
      2.1,
      -0.08,
      0.08,
      top - 0.16,
      mount,
      metal(AIRLOCK_TINT.yellow),
    );
  }
};

const suitLocker: InteriorRecipe = ({ k, kind }) => {
  const { hw, d1 } = interiorHalf(kind);
  const face = d1 - 0.01;
  k.box(
    -hw + 0.02,
    hw - 0.02,
    0.02,
    d1 - 0.04,
    0,
    0.08,
    metal(AIRLOCK_TINT.black),
  );
  k.bevelBox(
    -hw,
    hw,
    0,
    d1 - 0.04,
    0.08,
    2.28,
    0.02,
    metal(AIRLOCK_TINT.locker),
  );
  k.box(
    -0.38,
    0.38,
    d1 - 0.04,
    face,
    0.16,
    2.12,
    metal(AIRLOCK_TINT.lockerDoor),
  );
  k.panel(-0.2, 0.2, face + 0.005, 1.35, 1.9, {
    layer: LAYER.panel,
    tint: AIRLOCK_TINT.window,
    flag: FLAG.emissive,
  });
  k.box(0.24, 0.3, face, d1, 0.9, 1.2, metal(AIRLOCK_TINT.steel));
  for (const h of [0.3, 0.4, 0.5])
    k.panel(-0.25, 0.25, face + 0.005, h, h + 0.03, paint(AIRLOCK_TINT.dark));
  k.panel(-0.3, -0.1, face + 0.005, 2.0, 2.08, paint(AIRLOCK_TINT.yellow));
  k.panel(0.25, 0.31, face + 0.005, 2.0, 2.06, {
    layer: LAYER.panel,
    tint: AIRLOCK_TINT.green,
    flag: FLAG.signal,
  });
};

/** The airlock's fitting kinds' recipes. */
export const AIRLOCK_RECIPES = {
  "outer-hatch": outerHatch,
  beacon,
  "iris-light": irisLight,
  "suit-locker": suitLocker,
} satisfies Record<
  Extract<
    InteriorKind,
    "outer-hatch" | "beacon" | "iris-light" | "suit-locker"
  >,
  InteriorRecipe
>;
