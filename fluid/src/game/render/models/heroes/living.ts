/**
 * The living heroes' recipes: the mess table, the helper robot, the sleep
 * ring and the dome planters. What they share is the crew's life aboard:
 * long low tops and benches (the mess table, the planters' beds), rounded
 * shells (the robot's body, the sleep pods, the domes) and soft lights set
 * into them (the robot's face, the pods' ring, the grow lights).
 *
 * Every recipe reads its sizes from `heroHalf` or, where the brief gives an
 * exact number smaller than the catalogue box (the mess table's slab, the
 * bird toy, a pod's plinth), from that number directly: the catalogue box
 * is the outer limit a recipe stays inside, not a size it must fill.
 * Colours are the look's surfaces, or the named tints below for what no
 * palette carries (white furniture, glass, soil, plant green, a grow
 * lamp's glow, a steady signal and a pod's status light).
 */

import type { HeroKind } from "../../../world/types";
import { DECAL_LIFT, frameAt } from "../../kit";
import type { Rgb } from "../../looks";
import { profileAlong, sideways, yawed } from "../common";
import { heroHalf, type HeroRecipe } from "./common";

/** The mess table and its benches: a clean warm off-white (the brief's own numbers). */
const TABLE_WHITE: Rgb = [0.9, 0.9, 0.88];

/** The drinking bird's lower bulb: a deep red liquid. */
const RED_LIQUID: Rgb = [0.72, 0.1, 0.08];

/**
 * Every clear glass part in this file: the drinking bird's tube and head,
 * the dome planters' hemispheres and the sleep pods' lids, all one pale
 * tint so the station's glass reads the same wherever it appears.
 */
const GLASS_PALE: Rgb = [0.78, 0.88, 0.92];

/** The dome planters' soil. */
const SOIL_BROWN: Rgb = [0.32, 0.22, 0.15];

/** The dome planters' small plants. */
const PLANT_GREEN: Rgb = [0.16, 0.42, 0.18];

/** A grow lamp's underside glow: the pink-violet of a horticultural lamp. */
const GROW_TINT: Rgb = [1.0, 0.45, 0.75];

/** The helper robot's face pixels, both expressions: a friendly cyan LED. */
const FACE_GLOW: Rgb = [0.35, 0.95, 1.0];

/** A steady "all is well" light: the robot's antenna tip and the sleep ring's hub. */
const SIGNAL_TINT: Rgb = [0.55, 0.9, 1.0];

/** A sleep pod's status light. */
const POD_LIGHT: Rgb = [0.45, 0.8, 1.0];

/** The chamfer of this file's housings, in metres. */
const BEVEL = 0.02;

/**
 * The mess table: a long white top on two bevelled pedestals with foot
 * plates, and a bench each side on three supports. A drinking-bird toy (a
 * small glass novelty that dips and drinks) stands on the table by the
 * right pedestal, with a tray and two cups beside it; the catalogue's
 * surface (`a` -2.1 to 0.9) is left bare for a later milestone's curios.
 * The bank is steady: nothing on the table blinks.
 */
const messTable: HeroRecipe = ({ k, s }) => {
  const white = s.tinted(TABLE_WHITE);
  const glass = s.tinted(GLASS_PALE);

  // The top, on two pedestals with foot plates.
  k.bevelBox(-2.3, 2.3, -0.45, 0.45, 0.72, 0.76, BEVEL, white);
  for (const a of [-1.6, 1.6]) {
    k.bevelBox(a - 0.22, a + 0.22, -0.22, 0.22, 0, 0.05, BEVEL, white);
    k.bevelBox(a - 0.14, a + 0.14, -0.14, 0.14, 0.05, 0.72, BEVEL, white);
  }

  // Two long benches, three supports each.
  const BENCH_SUPPORTS = [-1.8, 0, 1.8] as const;
  for (const [d0, d1] of [
    [0.95, 1.25],
    [-1.25, -0.95],
  ] as const) {
    k.bevelBox(-2.2, 2.2, d0, d1, 0.42, 0.46, BEVEL, white);
    const dm = (d0 + d1) / 2;
    for (const a of BENCH_SUPPORTS)
      k.box(a - 0.05, a + 0.05, dm - 0.08, dm + 0.08, 0, 0.42, white);
  }

  // The drinking-bird toy at a 1.6, d 0: a base, two legs to a pivot bar,
  // a glass tube from a red lower bulb to a head bulb, a beak and a hat.
  const A = 1.6;
  k.cylinder(A, 0, 0.76, 0.78, 0.05, 10, s.dark);
  for (const da of [-0.025, 0.025])
    k.cylinder(A + da, 0, 0.78, 0.92, 0.005, 6, s.dark);
  k.cylinderAlong(A - 0.025, A + 0.025, 0, 0.92, 0.006, 6, s.dark);
  k.lathe(
    A,
    0,
    [
      [0, 0.795],
      [0.02, 0.805],
      [0.03, 0.82],
      [0.02, 0.835],
      [0, 0.845],
    ],
    8,
    s.tinted(RED_LIQUID),
  );
  k.lathe(
    A,
    0,
    [
      [0, 0.845],
      [0.012, 0.85],
      [0.012, 1.015],
      [0, 1.02],
    ],
    6,
    glass,
  );
  k.lathe(
    A,
    0,
    [
      [0, 1.015],
      [0.018, 1.025],
      [0.025, 1.04],
      [0.018, 1.055],
      [0, 1.065],
    ],
    8,
    glass,
  );
  k.box(A - 0.008, A + 0.008, 0.02, 0.05, 1.035, 1.045, s.dark);
  k.lathe(
    A,
    0,
    [
      [0, 1.06],
      [0.035, 1.065],
      [0.035, 1.07],
      [0.02, 1.07],
      [0.02, 1.1],
      [0, 1.1],
    ],
    8,
    s.dark,
  );

  // A tray and two cups, clear of the catalogue's surface (a -2.1 to 0.9).
  k.box(1.0, 1.3, -0.18, 0.18, 0.76, 0.775, s.dark);
  k.cylinder(1.08, -0.06, 0.775, 0.83, 0.035, 10, glass);
  k.cylinder(1.22, 0.07, 0.775, 0.83, 0.035, 10, glass);
};

/**
 * The helper robot: a track base of two treads on road wheels, a boxy
 * body, a neck and a head carrying a dark screen. Two short arms with claw
 * grippers sit at the body's sides, and a thin antenna runs from the head
 * to the catalogue's top with a steady signal tip. The screen shows block
 * pixels: face A (two round eyes, a smile) in the upper half in groups 0
 * to 3, face B (two closed eyes, a small open mouth) in the lower half in
 * groups 4 to 7, kept far enough apart in height that the two never share
 * a quad. The bank is swap: the faces alternate, one always dark.
 */
const helperRobot: HeroRecipe = ({ k, s, variant, kind }) => {
  const { d0, d1 } = heroHalf(kind, variant);

  // Track base: two treads, full depth, on four road wheels each.
  for (const [a0, a1] of [
    [-0.6, -0.35],
    [0.35, 0.6],
  ] as const) {
    k.bevelBox(a0, a1, d0, d1, 0, 0.3, BEVEL, s.dark);
    for (const d of [-0.525, -0.175, 0.175, 0.525])
      k.cylinderAlong(a0, a1, d, 0.15, 0.15, 8, s.metal);
  }

  // Body, neck and head, with the screen recessed on the head's front.
  k.bevelBox(-0.35, 0.35, -0.4, 0.4, 0.3, 1.1, BEVEL, s.body);
  k.box(-0.12, 0.12, -0.12, 0.12, 1.1, 1.16, s.dark);
  k.bevelBox(-0.3, 0.3, -0.2, 0.2, 1.16, 1.52, BEVEL, s.body);
  k.box(-0.22, 0.22, 0.16, 0.2, 1.2, 1.46, s.dark);

  // Two short arms with claw grippers.
  for (const side of [-1, 1] as const) {
    const arm0 = Math.min(side * 0.35, side * 0.52);
    const arm1 = Math.max(side * 0.35, side * 0.52);
    k.box(arm0, arm1, -0.05, 0.05, 0.55, 0.63, s.metal);
    const claw0 = Math.min(side * 0.5, side * 0.58);
    const claw1 = Math.max(side * 0.5, side * 0.58);
    k.box(claw0, claw1, -0.08, -0.02, 0.55, 0.64, s.dark);
    k.box(claw0, claw1, 0.02, 0.08, 0.55, 0.64, s.dark);
  }

  // A thin antenna to the catalogue's top, with a steady signal tip.
  k.cylinder(0, 0, 1.52, 1.58, 0.008, 6, s.dark);
  k.box(-0.02, 0.02, -0.02, 0.02, 1.58, 1.6, s.signal(SIGNAL_TINT));

  // Face A (groups 0-3): two round eyes and a smile, the screen's upper half.
  const screen = 0.2 + DECAL_LIFT;
  [-0.15, 0.15].forEach((a, i) => {
    k.panel(a - 0.03, a + 0.03, screen, 1.38, 1.44, s.blink(FACE_GLOW, i));
  });
  [-0.1, 0, 0.1].forEach((a, i) => {
    k.panel(
      a - 0.03,
      a + 0.03,
      screen,
      1.335,
      1.365,
      s.blink(FACE_GLOW, 2 + (i % 2)),
    );
  });

  // Face B (groups 4-7): two closed eyes and a small open mouth, the lower half.
  [-0.15, 0.15].forEach((a, i) => {
    k.panel(
      a - 0.045,
      a + 0.045,
      screen,
      1.245,
      1.255,
      s.blink(FACE_GLOW, 4 + i),
    );
  });
  k.panel(-0.035, 0.035, screen, 1.2, 1.23, s.blink(FACE_GLOW, 6));
};

/**
 * The sleep ring: a low round hub with a cap ring, a central column to its
 * catalogue top and a steady signal ring near the top. Six pods lie
 * radially round it at 60 degree steps, each a bevelled plinth under a
 * clear cylindrical lid with one status light on its hub-facing end. The
 * bank is chase: the six pod lights run round the ring one at a time, in
 * pod order.
 */
const sleepRing: HeroRecipe = ({ k, kitAt, s }) => {
  const f = frameAt([0, 0, 0], 0);

  // The hub: a low cylinder with a cap ring, a column and a steady ring.
  k.cylinder(0, 0, 0, 0.6, 0.7, 16, s.body);
  k.ring(0, 0, 0.6, 0.66, 0.03, 4, 20, s.metal, "up");
  k.cylinder(0, 0, 0.6, 1.4, 0.18, 10, s.metal);
  k.ring(0, 0, 1.28, 0.19, 0.02, 4, 16, s.signal(SIGNAL_TINT), "up");

  // Six pods, each yawed a further 60 degrees, lying radially outward.
  for (let pod = 0; pod < 6; pod++) {
    const fp = yawed(f, 0, 0, (pod * Math.PI) / 3);
    const kp = kitAt(fp);
    kp.bevelBox(-0.4, 0.4, 0.8, 2.4, 0, 0.5, BEVEL, s.body);
    kitAt(sideways(fp)).cylinderAlong(
      0.8,
      2.4,
      0,
      0.6,
      0.28,
      10,
      s.tinted(GLASS_PALE),
    );
    kp.box(-0.06, 0.06, 0.78, 0.86, 0.5, 0.56, s.blink(POD_LIGHT, pod));
  }
};

/** A dome's local `a` positions for `count` domes, `DOME_SPACING` metres apart, centred on 0. */
function domePositions(count: number): number[] {
  const spacing = 1.6;
  return Array.from(
    { length: count },
    (_, i) => (i - (count - 1) / 2) * spacing,
  );
}

/**
 * The dome planters: a raised bed with a soil mound running its length,
 * carrying two (variant 0) or three (variant 1) faceted glass domes over a
 * few small plants, spaced 1.6 m apart along `a`. Over each dome a thin
 * grow lamp stands from the bed's back edge to the catalogue's top, its
 * arm reaching over the dome's apex to a lamp head whose underside glows.
 * The bank is breathe: each lamp head is its dome's own group, so the two
 * or three lamps glow out of step with each other.
 */
const domePlanters: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const f = frameAt([0, 0, 0], 0);
  const positions = domePositions(variant === 0 ? 2 : 3);

  // The raised bed, the whole footprint less 0.05, with a soil mound.
  k.bevelBox(-(hw - 0.05), hw - 0.05, -0.75, 0.75, 0, 0.35, BEVEL, s.body);
  profileAlong(
    kitAt,
    f,
    [
      [-0.75, 0.3],
      [0.75, 0.3],
      [0.68, 0.34],
      [-0.68, 0.34],
    ],
    -(hw - 0.08),
    hw - 0.08,
    s.tinted(SOIL_BROWN),
  );

  positions.forEach((a, i) => {
    // A faceted glass dome, 6 sides, 3 latitude steps.
    k.lathe(
      a,
      0,
      [
        [0.7, 0.35],
        [0.65, 0.55],
        [0.45, 0.8],
        [0, 1.05],
      ],
      6,
      s.tinted(GLASS_PALE),
    );
    // A few small plants under the dome.
    k.lathe(
      a - 0.2,
      0.1,
      [
        [0, 0.35],
        [0.1, 0.4],
        [0.05, 0.52],
        [0, 0.58],
      ],
      6,
      s.tinted(PLANT_GREEN),
    );
    k.lathe(
      a + 0.15,
      -0.15,
      [
        [0, 0.35],
        [0.09, 0.38],
        [0.04, 0.5],
        [0, 0.55],
      ],
      6,
      s.tinted(PLANT_GREEN),
    );
    k.box(a - 0.05, a + 0.02, -0.25, -0.18, 0.35, 0.42, s.tinted(PLANT_GREEN));

    // The grow lamp: a post from the bed's back edge, an arm over the
    // dome's apex and a lamp head whose underside glows in the dome's group.
    k.cylinder(a, -0.75, 0, 1.5, 0.025, 6, s.metal);
    k.box(a - 0.02, a + 0.02, -0.75, 0, 1.46, 1.5, s.metal);
    k.cylinder(a, 0, 1.19, 1.46, 0.02, 6, s.metal);
    k.bevelBox(a - 0.1, a + 0.1, -0.1, 0.1, 1.09, 1.19, 0.015, s.dark);
    k.box(a - 0.08, a + 0.08, -0.08, 0.08, 1.06, 1.09, s.blink(GROW_TINT, i));
  });
};

/** The living kinds' recipes. */
export const LIVING_RECIPES = {
  "mess-table": messTable,
  "helper-robot": helperRobot,
  "sleep-ring": sleepRing,
  "dome-planters": domePlanters,
} satisfies Record<
  Extract<
    HeroKind,
    "mess-table" | "helper-robot" | "sleep-ring" | "dome-planters"
  >,
  HeroRecipe
>;
