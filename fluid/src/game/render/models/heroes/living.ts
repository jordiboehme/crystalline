/**
 * The living heroes' recipes: the mess table, the helper robot, the sleep
 * ring and the dome planters. What they share is the crew's life aboard:
 * clean white furniture and shells (the long mess table and its benches,
 * the robot's boxy body, the sleep pods, the planters' drums) and soft
 * lights set into them (the robot's pixel face, the pods' lit lids, the
 * grow lamps).
 *
 * Every recipe reads its sizes from `heroHalf` or, where a part is smaller
 * than the catalogue box (the mess table's top, the bird toy, a pod, a
 * dome), from its own numbers directly: the catalogue box is the outer
 * limit a recipe stays inside, not a size it must fill.
 *
 * The renderer draws no translucent surface, so glass is shown in two
 * ways: a lit pane that shines by itself reads as clear glass with a light
 * behind it (the sleep pods' lids), and an open strut lattice reads as a
 * clear dome because what is under it stays in view (the dome planters).
 * The drinking bird's glass is a plain pale tint, with its red liquid kept
 * outside every glass part so it never hides.
 *
 * Colours are the look's surfaces, or the named tints below for what no
 * palette carries: white furniture and shells, grey trim, glass, the red
 * liquid, the black hat, soil and plant greens, the lamps and screens.
 * Every look shows these heroes in the same colours on purpose: they are
 * recognised by them.
 */

import type { HeroKind } from "../../../world/types";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import type { Surface } from "../../geometry";
import {
  sideways,
  tiltedBar,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import { ALUMINIUM, heroHalf, type HeroRecipe } from "./common";

/** The mess table, its benches and the sleep pods' shells: a clean warm off-white. */
const TABLE_WHITE: Rgb = [0.9, 0.9, 0.88];

/** The mess table's grey trim: the skirt under its top, its foot and the tray. A soft gunmetal. */
const TRIM_GREY: Rgb = ALUMINIUM;

/** The drinking bird's red liquid, in its body bulb and the foot of its neck. */
const RED_LIQUID: Rgb = [0.75, 0.1, 0.1];

/** The drinking bird's top hat: black felt. */
const HAT_BLACK: Rgb = [0.05, 0.05, 0.05];

/** The drinking bird's stand, legs and pivot: bright chrome. */
const CHROME: Rgb = [0.75, 0.75, 0.77];

/**
 * Plain clear glass: the drinking bird's neck and head, its water glass
 * and the table's cups, all one pale tint.
 */
const GLASS_PALE: Rgb = [0.78, 0.88, 0.92];

/** The helper robot's body: a clean white plastic. */
const ROBOT_WHITE: Rgb = [0.9, 0.9, 0.88];

/** The helper robot's grey parts: its chassis, waist, hood, screen bezel and arms. */
const ROBOT_GREY: Rgb = [0.55, 0.56, 0.58];

/** The helper robot's screen behind the pixels, and its camera lens: near black. */
const SCREEN_BLACK: Rgb = [0.05, 0.05, 0.05];

/** The helper robot's face pixels, both expressions: a warm yellow line drawing. */
const FACE_GLOW: Rgb = [0.95, 0.85, 0.15];

/** A steady "all is well" light: the robot's antenna tip and the sleep ring's column ring. */
const SIGNAL_TINT: Rgb = [0.55, 0.9, 1.0];

/** A sleep pod's lid: pale blue glass lit softly from inside. */
const POD_GLOW: Rgb = [0.55, 0.75, 0.95];

/** A sleep pod's status light on its hub end, the one the chase runs round. */
const POD_LIGHT: Rgb = [0.45, 0.8, 1.0];

/** The dome planters' drums and base: a white hull with a touch of grey. */
const HULL_WHITE: Rgb = [0.8, 0.8, 0.82];

/** The dome planters' lattice struts: light grey aluminium. */
const STRUT_GREY: Rgb = [0.75, 0.75, 0.75];

/** The dome planters' soil and tree trunks. */
const SOIL_BROWN: Rgb = [0.35, 0.25, 0.15];

/** The dome planters' bright leaves: a rich mid green. */
const PLANT_GREEN: Rgb = [0.25, 0.45, 0.2];

/** The dome planters' deep leaves, the tree canopies: a darker green for depth. */
const LEAF_DARK: Rgb = [0.13, 0.33, 0.12];

/** A grow lamp's underside glow: the warm amber of a lamp that keeps a garden alive. */
const GROW_TINT: Rgb = [0.95, 0.7, 0.35];

/** The chamfer of this file's housings, in metres. */
const LIVING_BEVEL = 0.02;

/**
 * A flat stadium (a box with a half round at each end, seen from above)
 * from `a0` to `a1` along `a`, `2 * r` deep about `dm`: the box runs
 * between the two end centres and a cylinder of radius `r` stands at each.
 * The cylinders stop `1 mm` under the box's top, so their caps never fight
 * the box's top face where the two overlap; from outside the step is too
 * small to see. The mess table's top, its skirt and its benches.
 */
function stadium(
  k: Kit,
  a0: number,
  a1: number,
  dm: number,
  r: number,
  h0: number,
  h1: number,
  sides: number,
  s: Surface,
  bevel = 0,
): void {
  const c0 = a0 + r;
  const c1 = a1 - r;
  k.bevelBox(c0, c1, dm - r, dm + r, h0, h1, bevel, s);
  for (const c of [c0, c1]) k.cylinder(c, dm, h0, h1 - 0.001, r, sides, s);
}

/** Where the drinking bird stands on the mess table, along `a` (at `d` 0). */
export const BIRD_A = 1.15;

/**
 * The mess table: a very long white top with rounded ends over a grey
 * skirt, on one long white spine with a grey foot, and a long white bench
 * with rounded ends down each side on three white supports. A drinking-bird toy
 * (a glass novelty that dips and drinks) stands on the table at `BIRD_A`:
 * a chrome stand with two legs and a pivot, a red liquid body bulb, a neck
 * red at its foot and clear above, a clear head bulb with a beak, a black
 * top hat, and a glass of water in front of its beak. A tray and two cups
 * sit near the right end. The catalogue's surface (`a` -2.1 to 0.9, `d`
 * -0.4 to 0.4, at 0.76) lies on the top's flat face, left bare for a later
 * milestone's curios. The bank is steady: nothing on the table blinks.
 */
const messTable: HeroRecipe = ({ k, s }) => {
  const white = s.tinted(TABLE_WHITE);
  const trim = s.tinted(TRIM_GREY);
  const glass = s.tinted(GLASS_PALE);
  const chrome = s.tinted(CHROME);

  // The top with rounded ends, a grey skirt under it, a spine and a foot.
  stadium(k, -2.38, 2.38, 0, 0.48, 0.72, 0.76, 16, white, LIVING_BEVEL);
  stadium(k, -2.2, 2.2, 0, 0.4, 0.66, 0.72, 12, trim);
  k.bevelBox(-1.5, 1.5, -0.16, 0.16, 0.05, 0.66, LIVING_BEVEL, white);
  k.bevelBox(-1.65, 1.65, -0.3, 0.3, 0, 0.05, LIVING_BEVEL, trim);

  // A bench down each side: a white seat with rounded ends on three
  // white supports.
  for (const dm of [1.13, -1.13]) {
    stadium(k, -2.15, 2.15, dm, 0.13, 0.38, 0.45, 10, white, LIVING_BEVEL);
    for (const a of [-1.6, 0, 1.6])
      k.bevelBox(
        a - 0.1,
        a + 0.1,
        dm - 0.08,
        dm + 0.08,
        0,
        0.38,
        LIVING_BEVEL,
        white,
      );
  }

  // The drinking bird: a chrome stand, two legs and a pivot bar.
  const A = BIRD_A;
  k.cylinder(A, 0, 0.76, 0.77, 0.045, 10, chrome);
  for (const da of [-0.03, 0.03])
    k.cylinder(A + da, 0, 0.77, 0.93, 0.005, 6, chrome);
  k.cylinderAlong(A - 0.035, A + 0.035, 0, 0.93, 0.006, 6, chrome);
  // The red body bulb and the red foot of its neck, then the clear neck.
  k.lathe(
    A,
    0,
    [
      [0, 0.79],
      [0.022, 0.797],
      [0.032, 0.815],
      [0.03, 0.835],
      [0.012, 0.852],
      [0.01, 0.9],
      [0, 0.9],
    ],
    10,
    s.tinted(RED_LIQUID),
  );
  k.lathe(
    A,
    0,
    [
      [0, 0.9],
      [0.01, 0.9],
      [0.01, 1.02],
      [0, 1.025],
    ],
    6,
    glass,
  );
  // The clear head bulb, its beak and the black top hat.
  k.lathe(
    A,
    0,
    [
      [0, 1.015],
      [0.018, 1.022],
      [0.025, 1.038],
      [0.018, 1.054],
      [0, 1.062],
    ],
    8,
    glass,
  );
  k.box(A - 0.007, A + 0.007, 0.02, 0.055, 1.032, 1.042, s.dark);
  k.lathe(
    A,
    0,
    [
      [0, 1.058],
      [0.038, 1.058],
      [0.038, 1.064],
      [0.021, 1.064],
      [0.022, 1.1],
      [0, 1.1],
    ],
    10,
    s.tinted(HAT_BLACK),
  );
  // The glass of water it dips its beak towards.
  k.cylinder(A, 0.1, 0.76, 0.84, 0.03, 10, glass);

  // A tray and two cups near the right end, clear of the surface and the bird.
  k.box(1.45, 1.75, -0.18, 0.18, 0.76, 0.775, trim);
  k.cylinder(1.53, -0.06, 0.775, 0.83, 0.035, 10, glass);
  k.cylinder(1.67, 0.07, 0.775, 0.83, 0.035, 10, glass);
};

/**
 * The helper robot: a boxy white body with a grey hood on a grey waist and
 * chassis, riding two dark tank treads on road wheels. Its front carries a
 * square screen in a grey bezel, near black behind block pixels, with a
 * small dark camera lens beside it. Two short grey arms reach forward from
 * its sides to small grippers, and a thin antenna on the hood ends in a
 * steady signal tip at the catalogue's top.
 *
 * The screen shows one of two yellow faces at a time, both filling its
 * middle: face A (groups 0 to 3) two square eyes and a wide smile, face B
 * (groups 4 to 7) two closed eyes and a small round mouth. Their pixels
 * interleave so that no quad of one overlaps a quad of the other. The
 * bank is swap: the faces alternate, one always dim.
 */
const helperRobot: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { d0, d1 } = heroHalf(kind, variant);
  const white = s.tinted(ROBOT_WHITE);
  const grey = s.tinted(ROBOT_GREY);
  const black = s.tinted(SCREEN_BLACK);

  // Two treads, nearly full depth, on three road wheels each.
  for (const [a0, a1] of [
    [-0.58, -0.36],
    [0.36, 0.58],
  ] as const) {
    k.bevelBox(a0, a1, d0 + 0.08, d1 - 0.08, 0, 0.28, 0.04, s.dark);
    for (const d of [-0.38, 0, 0.38])
      k.cylinderAlong(a0 - 0.01, a1 + 0.01, d, 0.14, 0.1, 8, s.metal);
  }

  // The chassis, the waist, the white body and its grey hood.
  k.bevelBox(-0.4, 0.4, -0.5, 0.5, 0.2, 0.34, LIVING_BEVEL, grey);
  k.bevelBox(-0.32, 0.32, -0.36, 0.38, 0.34, 0.5, LIVING_BEVEL, grey);
  k.bevelBox(-0.42, 0.42, -0.42, 0.46, 0.5, 1.42, 0.03, white);
  k.bevelBox(-0.44, 0.44, -0.44, 0.48, 1.42, 1.5, LIVING_BEVEL, grey);

  // The square screen: a grey bezel, the black glass on it.
  k.box(-0.3, 0.3, 0.46, 0.48, 0.78, 1.36, grey);
  k.box(-0.26, 0.26, 0.48, 0.49, 0.82, 1.32, black);

  // The camera lens beside the screen, looking out of the front. In the
  // sideways frame `a'` runs along `-d` and `d'` along `a`.
  kitAt(sideways(frameAt([0, 0, 0], 0))).cylinderAlong(
    -0.5,
    -0.46,
    0.35,
    1.26,
    0.035,
    8,
    black,
  );

  // Two short arms: a shoulder out of each side, a forearm reaching
  // forward and a two-finger gripper.
  for (const side of [-1, 1] as const) {
    const span = (x: number, y: number) =>
      [Math.min(side * x, side * y), Math.max(side * x, side * y)] as const;
    const [s0, s1] = span(0.42, 0.55);
    k.box(s0, s1, -0.06, 0.06, 0.8, 0.9, grey);
    const [f0, f1] = span(0.47, 0.55);
    k.box(f0, f1, 0.06, 0.34, 0.81, 0.89, grey);
    k.box(f0, f1, 0.34, 0.42, 0.87, 0.9, s.dark);
    k.box(f0, f1, 0.34, 0.42, 0.8, 0.83, s.dark);
  }

  // A thin antenna on the hood, with a steady signal tip.
  k.cylinder(0.3, -0.3, 1.5, 1.58, 0.008, 6, s.dark);
  k.box(0.28, 0.32, -0.32, -0.28, 1.58, 1.6, s.signal(SIGNAL_TINT));

  const screen = 0.49 + DECAL_LIFT;
  const pixel = (a0: number, a1: number, h0: number, h1: number, g: number) => {
    k.panel(a0, a1, screen, h0, h1, s.blink(FACE_GLOW, g));
  };
  // Face A (groups 0-3): two square eyes, a smile's bottom and its corners.
  pixel(-0.15, -0.07, 1.14, 1.22, 0);
  pixel(0.07, 0.15, 1.14, 1.22, 1);
  pixel(-0.1, 0.1, 0.92, 0.96, 2);
  pixel(-0.15, -0.1, 0.96, 1.0, 3);
  pixel(0.1, 0.15, 0.96, 1.0, 3);
  // Face B (groups 4-7): two closed eyes under A's, a small round mouth
  // between A's smile and B's eyes.
  pixel(-0.16, -0.06, 1.08, 1.11, 4);
  pixel(0.06, 0.16, 1.08, 1.11, 5);
  pixel(-0.04, 0.04, 1.01, 1.06, 6);
};

/** How far a pod's shell reaches from the ring's centre, and how wide it is. */
const POD = { start: 0.6, end: 2.0, half: 0.38, top: 0.42 } as const;

/**
 * One sleep pod, number `pod` (0 to 5), built into the ring's frame `f`:
 * the frame yawed `pod` sixths of a turn, the pod lying outward along its
 * `d`, so its outward heading in `f` is `(-sin, cos)` of `pod * 60`
 * degrees in `(a, d)`. A white shell with a rounded outer end, a soft blue lit lid
 * with a domed outer end over it, and the status light (blink group
 * `pod`) on the shell's top at the hub end. Exported so a test can build a
 * single pod and check its lid lies over its own shell.
 */
export function sleepPod(
  kitAt: KitAt,
  f: Frame,
  s: Surfaces,
  pod: number,
): void {
  const white = s.tinted(TABLE_WHITE);
  const lid = s.signal(POD_GLOW);
  const fp = yawed(f, 0, 0, (pod * Math.PI) / 3);
  const kp = kitAt(fp);
  // The shell, its rounded outer end.
  kp.bevelBox(-POD.half, POD.half, POD.start, POD.end, 0, POD.top, 0.04, white);
  kp.cylinder(0, POD.end, 0, POD.top - 0.001, POD.half, 12, white);
  // The lid along the pod: in the sideways frame `a'` runs along `-d`,
  // so the pod's `d` from 0.88 to `POD.end` is `a'` from `-POD.end` to
  // -0.88. The lid's lower half sinks into the shell.
  kitAt(sideways(fp)).cylinderAlong(-POD.end, -0.88, 0, POD.top, 0.28, 10, lid);
  kp.lathe(
    0,
    POD.end,
    [
      [0, POD.top - 0.1],
      [0.28, POD.top],
      [0.2, POD.top + 0.2],
      [0, POD.top + 0.28],
    ],
    10,
    lid,
  );
  // The status light on the shell's top at the hub end.
  kp.box(
    -0.06,
    0.06,
    0.76,
    0.84,
    POD.top,
    POD.top + 0.05,
    s.blink(POD_LIGHT, pod),
  );
}

/**
 * The sleep ring: six white pods lying like the petals of a flower round a
 * round white hub. The hub is a low drum with a rounded shoulder and a
 * white column up to the catalogue's top, banded in grey, with a steady
 * signal ring near its head. Each pod is a white shell with a rounded
 * outer end, under a long lid with a domed outer end; the lid is a soft
 * blue pane lit from inside (the renderer has no clear glass, so a lit pane
 * is how a clear lid with a light under it reads). One status light sits
 * on each pod's hub end. The bank is chase: the six status lights run
 * round the ring one at a time, in pod order; the lids glow steadily.
 */
const sleepRing: HeroRecipe = ({ k, kitAt, s }) => {
  const f = frameAt([0, 0, 0], 0);
  const white = s.tinted(TABLE_WHITE);

  // The hub: a drum with a rounded shoulder, a banded column and a ring.
  k.lathe(
    0,
    0,
    [
      [0, 0],
      [0.72, 0],
      [0.72, 0.4],
      [0.62, 0.52],
      [0.3, 0.56],
      [0, 0.56],
    ],
    16,
    white,
  );
  k.cylinder(0, 0, 0.56, 1.4, 0.16, 10, white);
  for (const h of [0.7, 1.12]) k.cylinder(0, 0, h, h + 0.04, 0.17, 10, s.metal);
  k.ring(0, 0, 1.3, 0.17, 0.02, 4, 16, s.signal(SIGNAL_TINT), "up");

  // Six pods, each yawed a further 60 degrees, lying radially outward.
  for (let pod = 0; pod < 6; pod++) sleepPod(kitAt, f, s, pod);
};

/** The planters' dome: its sphere's radius, the drum top it sits on and the lattice's strut width. */
const DOME = { r: 0.7255, base: 0.42, strut: 0.025 } as const;

/** A dome's local `a` positions for `count` domes, 1.6 m apart, centred on 0. */
function domePositions(count: number): number[] {
  const spacing = 1.6;
  return Array.from(
    { length: count },
    (_, i) => (i - (count - 1) / 2) * spacing,
  );
}

/**
 * One straight strut of `width` square from `p` to `q` (each `[a, d, h]`
 * in `f`'s terms): a bar extruded in the vertical plane through both
 * points, so it may run at any slope and heading. The bar runs a strut's
 * width past each end so struts meeting at a joint close it. Kept local to
 * this file: the lattice is its only user.
 */
function strut(
  kitAt: KitAt,
  f: Frame,
  p: readonly [number, number, number],
  q: readonly [number, number, number],
  width: number,
  s: Surface,
): void {
  const da = q[0] - p[0];
  const dd = q[1] - p[1];
  const run = Math.hypot(da, dd);
  const rise = q[2] - p[2];
  const fs = yawed(f, p[0], p[1], Math.atan2(dd, da));
  kitAt(fs).extrude(
    tiltedBar(
      run / 2,
      (p[2] + q[2]) / 2,
      Math.atan2(rise, run),
      Math.hypot(run, rise) + width,
      width,
    ),
    -width / 2,
    width / 2,
    s,
  );
}

/**
 * The top of a one-frequency geodesic sphere, the icosahedron's own: an
 * apex, an upper ring of five and a lower ring of five turned a fifth of a
 * half turn, on a sphere of radius `DOME.r` whose lower ring stands on the
 * drum at `DOME.base`. Returns the struts from the apex to the upper ring,
 * round the upper ring and in a zigzag down to the lower ring; the lower
 * ring itself is the drum's collar. Pure in the dome's centre `a`.
 */
function domeStruts(
  a: number,
): [readonly [number, number, number], readonly [number, number, number]][] {
  const lift = DOME.r / Math.sqrt(5);
  const across = (2 * DOME.r) / Math.sqrt(5);
  const centre = DOME.base + lift;
  const apex = [a, 0, centre + DOME.r] as const;
  const ring = (i: number, turn: number, h: number) =>
    [
      a + across * Math.cos(((2 * i + turn) * Math.PI) / 5),
      across * Math.sin(((2 * i + turn) * Math.PI) / 5),
      h,
    ] as const;
  const out: [
    readonly [number, number, number],
    readonly [number, number, number],
  ][] = [];
  for (let i = 0; i < 5; i++) {
    const up = ring(i, 0, centre + lift);
    out.push([apex, up]);
    out.push([up, ring(i + 1, 0, centre + lift)]);
    out.push([up, ring(i, 1, DOME.base)]);
    out.push([up, ring(i - 1, 1, DOME.base)]);
  }
  return out;
}

/** The height of a dome's apex: `DOME.base` plus the icosahedron's cap height. */
const DOME_APEX = DOME.base + DOME.r / Math.sqrt(5) + DOME.r;

/**
 * The dome planters: a low white base the whole length of the footprint
 * carrying two (variant 0) or three (variant 1) gardens 1.6 m apart along
 * `a`. Each garden is a white drum with a grey collar and a soil bed,
 * planted with a small tree and a ring of dense shrubs in two greens,
 * under an open geodesic lattice of light grey struts (the icosahedron's
 * cap: twenty struts meeting at a white apex hub). The renderer has no
 * clear glass, so the dome is left unglazed: its openness is what reads
 * as clear, and the garden stays in view. From each apex hub a rod hangs
 * a grow lamp over the tree whose underside glows warm amber. The bank is
 * breathe: each lamp is its dome's own group, so the lamps glow out of
 * step with each other.
 */
const domePlanters: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const f = frameAt([0, 0, 0], 0);
  const hull = s.tinted(HULL_WHITE);
  const struts = s.tinted(STRUT_GREY);
  const soil = s.tinted(SOIL_BROWN);
  const green = s.tinted(PLANT_GREEN);
  const deep = s.tinted(LEAF_DARK);

  // The long low base.
  k.bevelBox(-(hw - 0.05), hw - 0.05, -0.55, 0.55, 0, 0.1, LIVING_BEVEL, hull);

  domePositions(variant === 0 ? 2 : 3).forEach((a, i) => {
    // The drum, its collar and the soil bed, which also caps the collar
    // (the drum and collar are open tubes: their ends are never seen).
    k.cylinder(a, 0, 0.1, 0.36, 0.68, 10, hull, false);
    k.cylinder(a, 0, 0.36, DOME.base, 0.7, 10, s.metal, false);
    k.cylinder(a, 0, DOME.base, DOME.base + 0.02, 0.7, 10, soil);

    // The garden: a small tree in the middle, dense shrubs round it.
    const bed = DOME.base + 0.02;
    k.cylinder(a, 0, bed, bed + 0.36, 0.03, 5, soil, false);
    k.lathe(
      a,
      0,
      [
        [0, bed + 0.26],
        [0.28, bed + 0.36],
        [0.3, bed + 0.5],
        [0.18, bed + 0.64],
        [0, bed + 0.68],
      ],
      6,
      deep,
    );
    for (let j = 0; j < 5; j++) {
      const t = ((2 * j + 1) * Math.PI) / 5;
      const [sa, sd] = [a + 0.4 * Math.cos(t), 0.4 * Math.sin(t)];
      const tall = j % 2 === 0 ? 0.34 : 0.26;
      k.lathe(
        sa,
        sd,
        [
          [0, bed],
          [0.2, bed + 0.08],
          [0.16, bed + tall * 0.75],
          [0, bed + tall],
        ],
        5,
        j % 2 === 0 ? green : deep,
      );
    }

    // The open lattice and its apex hub.
    for (const [p, q] of domeStruts(a))
      strut(kitAt, f, p, q, DOME.strut, struts);
    k.cylinder(a, 0, DOME_APEX - 0.04, 1.5, 0.045, 6, hull);

    // The grow lamp hanging from the hub: a rod, a housing and its glow.
    k.cylinder(a, 0, 1.2, DOME_APEX - 0.04, 0.012, 6, s.metal, false);
    k.box(a - 0.1, a + 0.1, -0.1, 0.1, 1.2, 1.25, s.dark);
    k.box(a - 0.08, a + 0.08, -0.08, 0.08, 1.18, 1.2, s.blink(GROW_TINT, i));
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
