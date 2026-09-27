/**
 * The critters' recipes (2.6d): the hover drone and the soot puffs.
 * Colours and helpers stay in this file, which imports only `common.ts`
 * of the curio batches. Nothing moves and neither carries any text.
 *
 * - The hover drone is built from its lift up (`CURIO_LIFT`), its middle
 *   `DRONE_MID` over its surface, facing `+d`. A dark round core
 *   (`CORE_DARK`, a lathe) sits inside a faceted shell of four angular
 *   quarters that together make a four-pointed star seen from the front,
 *   its points up, down and to the sides. The quarters stand apart along
 *   a plus-shaped seam and round a round eye socket, so the dark core
 *   shows between them. Each quarter is stepped through five layers
 *   (`LAYERS`): a wide middle plate in `SHELL_GREY`, two smaller, paler
 *   plates in front and two smaller, darker plates behind, which taper
 *   the shell to its front and its back like a double pyramid, each
 *   quarter's pale plates their own shade so the facets read apart. The
 *   core is smaller than the socket by 7 mm, so the shell never touches
 *   it but through the pegs. Four dark seam pegs lie in the plus
 *   seam, set back from the face, joining each quarter to the core. On
 *   the front a pale metal barrel reaches out of the core and carries the
 *   eye: a large round blue-white disc (`EYE`) with a brighter centre
 *   (`EYE_CENTRE`), both breathing in group 0 of the `breathe` bank. The
 *   bottom point is the lift and the top point the curio's top.
 * - The soot puffs huddle on the floor, three in v0 and five in v1
 *   (`PUFFS`). Each is one lathe in `SOOT_BLACK`: a ball whose radius
 *   alternates between the ball's own and `TUFT` times it at every step
 *   up its profile, so its fuzz is rings of low-poly ridges. Two crossed
 *   star plates in a near black (`FUZZ`) give each ball a spiky outline
 *   from the front and from the side. On its front at two thirds of its
 *   height, two big `EYE_WHITE` eyes set into the fuzz, each with a
 *   `PUPIL_BLACK` pupil. A few `CRUMB` specks lie on the floor round the
 *   huddle. No glow.
 */

import type { CurioKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import { discOutline, yawed, type KitAt } from "../common";
import { curioHalf, type CurioRecipe } from "./common";

/** Every critter is built in this frame, at the origin. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

// --- Hover drone -------------------------------------------------------------

/** The shell's middle plates: a plain grey metal. */
export const SHELL_GREY: Rgb = [0.55, 0.55, 0.58];

/** The core and the seam pegs: a dark gunmetal. */
export const CORE_DARK: Rgb = [0.2, 0.2, 0.22];

/** The layers in front of the middle plates: each quarter its own pale shade, so the facets read apart. */
const FACE_GREYS: readonly Rgb[] = [
  [0.66, 0.66, 0.69],
  [0.62, 0.62, 0.65],
  [0.6, 0.6, 0.63],
  [0.64, 0.64, 0.67],
];

/** The layers behind the middle plates: the shell's grey in shade, darker to the back. */
const BACK_GREYS: readonly Rgb[] = [
  [0.47, 0.47, 0.5],
  [0.4, 0.4, 0.43],
];

/** The eye's barrel: a pale bright metal ring round the lens. */
const BARREL: Rgb = [0.78, 0.79, 0.82];

/** The eye's glow: blue-white. */
const EYE: Rgb = [0.6, 0.85, 1.0];

/** The eye's brighter centre. */
const EYE_CENTRE: Rgb = [0.85, 0.95, 1.0];

/** The drone's middle over its surface: half way between its lift (0.35) and its top (0.53). */
const DRONE_MID = 0.44;

/** The core's radius, its facets and its profile's steps from pole to pole. */
const CORE_R = 0.035;
const CORE_SIDES = 8;
const CORE_STEPS = 5;

/** Half the plus-shaped seam between the quarters, in metres. */
const SEAM = 0.005;

/**
 * One layer of the shell: its eye socket's radius, the reach of its
 * points from the middle, how far its outer edge reaches at the diagonal
 * (a little past the straight line between the points, so the star's
 * points stay sharp) and its depth range along `d`.
 */
interface Layer {
  socket: number;
  reach: number;
  bulge: number;
  d0: number;
  d1: number;
}

/**
 * The shell's five layers from the front to the back: each quarter is
 * stepped through them like a double pyramid, widest in the middle
 * (`MIDDLE_LAYER`, its points on the curio's box), its sockets 7 mm clear
 * of the core, the back one closing behind the core.
 */
const LAYERS: readonly Layer[] = [
  { socket: 0.043, reach: 0.068, bulge: 0.038, d0: 0.042, d1: 0.062 },
  { socket: 0.042, reach: 0.079, bulge: 0.043, d0: 0.018, d1: 0.042 },
  { socket: 0.042, reach: 0.09, bulge: 0.048, d0: -0.018, d1: 0.018 },
  { socket: 0.04, reach: 0.07, bulge: 0.038, d0: -0.045, d1: -0.018 },
  { socket: 0.012, reach: 0.044, bulge: 0.024, d0: -0.07, d1: -0.045 },
];

/** The middle layer's index in `LAYERS`: its plates are `SHELL_GREY`. */
const MIDDLE_LAYER = 2;

/** The eye barrel's radius and its front; the eye disc's radius and depth; the centre's. */
const BARREL_R = 0.034;
const BARREL_FRONT = 0.07;
const EYE_R = 0.03;
const EYE_PROUD = 0.004;
const CENTRE_R = 0.012;
const CENTRE_PROUD = 0.001;

/** The seam pegs: their half thickness, their reach out from the middle and their depth range. */
const PEG_HALF = 0.003;
const PEG_REACH = 0.075;
const PEG_D0 = -0.04;
const PEG_D1 = 0.036;

/**
 * One quarter of a shell layer in the `(a, h)` plane round the middle,
 * mirrored by `sa` and `sh` (each 1 or -1): from the seam up to its
 * point, out along its bulging edge to its side point, back along the
 * seam and round the eye socket in two facets.
 */
function quarter(L: Layer, sa: number, sh: number): [number, number][] {
  const inner = Math.sqrt(L.socket * L.socket - SEAM * SEAM);
  const diag = L.socket / Math.SQRT2;
  return [
    [SEAM, inner],
    [SEAM, L.reach],
    [L.bulge, L.bulge],
    [L.reach, SEAM],
    [inner, SEAM],
    [diag, diag],
  ].map(([x = 0, y = 0]) => [sa * x, DRONE_MID + sh * y]);
}

/** The four quarters' mirror signs: upper right, upper left, lower left, lower right. */
const QUARTERS = [
  [1, 1],
  [-1, 1],
  [-1, -1],
  [1, -1],
] as const;

const hoverDrone: CurioRecipe = ({ k, s }) => {
  const dark = s.tinted(CORE_DARK);
  // The core: a dark faceted ball in the middle.
  const core: [number, number][] = Array.from(
    { length: CORE_STEPS + 1 },
    (_, i) => {
      const t = -Math.PI / 2 + (Math.PI * i) / CORE_STEPS;
      const pole = i === 0 || i === CORE_STEPS;
      return [
        pole ? 0 : CORE_R * Math.cos(t),
        DRONE_MID + CORE_R * Math.sin(t),
      ];
    },
  );
  k.lathe(0, 0, core, CORE_SIDES, dark);
  // The shell: four quarters, each stepped through the layers.
  QUARTERS.forEach(([sa, sh], i) => {
    LAYERS.forEach((L, j) => {
      const tint =
        j === MIDDLE_LAYER
          ? SHELL_GREY
          : j < MIDDLE_LAYER
            ? (FACE_GREYS[(i + j) % FACE_GREYS.length] ?? SHELL_GREY)
            : (BACK_GREYS[j - MIDDLE_LAYER - 1] ?? SHELL_GREY);
      k.extrude(quarter(L, sa, sh), L.d0, L.d1, s.tinted(tint));
    });
  });
  // The seam pegs, in the plus seam from the core out, set back from the face.
  for (const sign of [-1, 1]) {
    k.box(
      sign * (CORE_R - 0.01),
      sign * PEG_REACH,
      PEG_D0,
      PEG_D1,
      DRONE_MID - PEG_HALF,
      DRONE_MID + PEG_HALF,
      dark,
    );
    k.box(
      -PEG_HALF,
      PEG_HALF,
      PEG_D0,
      PEG_D1,
      DRONE_MID + sign * (CORE_R - 0.01),
      DRONE_MID + sign * PEG_REACH,
      dark,
    );
  }
  // The eye: a pale barrel out of the core, the glowing lens on it and
  // its brighter centre.
  k.extrude(
    discOutline(0, DRONE_MID, BARREL_R, 12),
    0,
    BARREL_FRONT,
    s.tinted(BARREL),
  );
  k.extrude(
    discOutline(0, DRONE_MID, EYE_R, 12),
    BARREL_FRONT - 0.001,
    BARREL_FRONT + EYE_PROUD,
    s.blink(EYE, 0),
  );
  k.extrude(
    discOutline(0, DRONE_MID, CENTRE_R, 8),
    BARREL_FRONT + EYE_PROUD - 0.0005,
    BARREL_FRONT + EYE_PROUD + CENTRE_PROUD,
    s.blink(EYE_CENTRE, 0),
  );
};

// --- Soot puffs --------------------------------------------------------------

/** The puffs' bodies: a sooty black. */
export const SOOT_BLACK: Rgb = [0.04, 0.04, 0.05];

/** The fuzz plates round each body: a black a hair lighter, so its spikes catch a little light. */
const FUZZ: Rgb = [0.06, 0.06, 0.07];

/** The eyes' whites. */
export const EYE_WHITE: Rgb = [0.95, 0.95, 0.92];

/** The pupils. */
export const PUPIL_BLACK: Rgb = [0.02, 0.02, 0.02];

/** The crumbs on the floor. */
export const CRUMB: Rgb = [0.08, 0.08, 0.09];

/** How far a ridge of fuzz reaches out, as a share of the ball's radius. */
const TUFT = 1.25;

/** The body lathe's facets and its profile's steps from bottom to top. */
const PUFF_SIDES = 7;
const PUFF_STEPS = 6;

/** The spikes of a fuzz plate, and their reach as a share of the ball's radius. */
const SPIKES = 9;
const SPIKE_REACH = 1.3;
const SPIKE_ROOT = 0.85;

/** A fuzz plate's half thickness. */
const FUZZ_HALF = 0.004;

/** An eye's side, how far apart the two eyes stand, and a pupil's side. */
const EYE_SIDE = 0.014;
const EYE_APART = 0.0085;
const PUPIL = 0.006;

/** How far an eye's white stands in front of its ball's radius, and how deep it is set in. */
const EYE_OUT = 0.003;
const EYE_DEEP = 0.01;

/**
 * One puff: its centre `(a, d)`, its radius, its yaw (a small turn so
 * the huddle looks about) and where its pupils look, `-1` to `1` of the
 * room the white leaves them.
 */
interface Puff {
  a: number;
  d: number;
  r: number;
  yaw: number;
  look: readonly [number, number];
}

/**
 * The huddles: v0 three puffs, v1 five, each inside its variant's box
 * with its fuzz; the tallest reaches the box's top.
 */
const PUFFS: readonly (readonly Puff[])[] = [
  [
    { a: -0.052, d: -0.006, r: 0.03, yaw: 0.25, look: [0.6, -0.3] },
    { a: 0.012, d: 0.012, r: 0.035, yaw: 0, look: [-0.2, -0.5] },
    { a: 0.068, d: -0.012, r: 0.026, yaw: -0.3, look: [-0.7, 0.2] },
  ],
  [
    { a: -0.11, d: 0.01, r: 0.028, yaw: 0.35, look: [0.6, 0] },
    { a: -0.05, d: -0.02, r: 0.034, yaw: 0.15, look: [0.3, -0.4] },
    { a: 0.012, d: 0.02, r: 0.04, yaw: 0, look: [0, -0.6] },
    { a: 0.075, d: -0.015, r: 0.031, yaw: -0.2, look: [-0.5, 0.2] },
    { a: 0.125, d: 0.028, r: 0.025, yaw: -0.4, look: [-0.8, -0.2] },
  ],
];

/** The crumbs, `[a, d, side]`, each on the floor round its huddle. */
const CRUMBS: readonly (readonly (readonly [number, number, number])[])[] = [
  [
    [-0.095, 0.045, 0.006],
    [-0.02, 0.055, 0.005],
    [0.05, 0.05, 0.007],
    [0.098, 0.03, 0.005],
    [-0.09, -0.045, 0.006],
  ],
  [
    [-0.155, 0.06, 0.006],
    [-0.08, 0.075, 0.005],
    [0.04, 0.08, 0.007],
    [0.105, 0.07, 0.006],
    [0.16, -0.03, 0.005],
    [-0.15, -0.06, 0.008],
  ],
];

/** The crumbs' height. */
const CRUMB_H = 0.004;

/**
 * A spiky star outline in the `(a, h)` plane round `(0, r)`: `SPIKES`
 * points reaching `SPIKE_REACH * r` between roots at `SPIKE_ROOT * r`,
 * kept between 3 mm over the floor and `top`, so the fuzz never dips
 * under the floor or past the curio's top. With `bareFront` the points
 * on the upper front (`+a` of the plate, where the side plate's `a` is
 * the puff's front) stay at the root, so no spike stands before the eyes.
 */
function fuzzOutline(
  r: number,
  top: number,
  turn: number,
  bareFront: boolean,
): [number, number][] {
  return Array.from({ length: 2 * SPIKES }, (_, i) => {
    const t = turn + (Math.PI * i) / SPIKES;
    const face = bareFront && Math.cos(t) > 0.2 && Math.sin(t) > -0.2;
    const reach = (i % 2 === 0 && !face ? SPIKE_REACH : SPIKE_ROOT) * r;
    const h = Math.min(top, Math.max(0.003, r + reach * Math.sin(t)));
    return [reach * Math.cos(t), h] as [number, number];
  });
}

/** One puff in its own yawed frame: its body, its fuzz, its eyes and pupils. */
function puff(
  kitAt: KitAt,
  p: Puff,
  top: number,
  surf: {
    body: Surface;
    fuzz: Surface;
    white: Surface;
    pupil: Surface;
  },
): void {
  const k: Kit = kitAt(yawed(ORIGIN, p.a, p.d, p.yaw));
  const { r } = p;
  // The body: a ball whose radius steps between its own and a ridge's.
  const profile: [number, number][] = [[0, 0]];
  for (let i = 1; i < PUFF_STEPS; i++) {
    const t = -Math.PI / 2 + (Math.PI * i) / PUFF_STEPS;
    const ridge = i % 2 === 1 ? TUFT : 1;
    profile.push([ridge * r * Math.cos(t), r + r * Math.sin(t)]);
  }
  profile.push([0, 2 * r]);
  k.lathe(0, 0, profile, PUFF_SIDES, surf.body);
  // The fuzz: two crossed spiky plates, one facing the front, one the side.
  k.extrude(fuzzOutline(r, top, 0.1, false), -FUZZ_HALF, FUZZ_HALF, surf.fuzz);
  kitAt(yawed(ORIGIN, p.a, p.d, p.yaw + Math.PI / 2)).extrude(
    fuzzOutline(r, top, 0.25, true),
    -FUZZ_HALF,
    FUZZ_HALF,
    surf.fuzz,
  );
  // The eyes, at two thirds of its height, set into the fuzz.
  const h = (4 * r) / 3;
  const e = EYE_SIDE / 2;
  const wf = r + EYE_OUT;
  for (const side of [-1, 1]) {
    const a = side * EYE_APART;
    k.box(a - e, a + e, wf - EYE_DEEP, wf, h - e, h + e, surf.white);
    const pa = a + p.look[0] * (e - PUPIL / 2);
    const ph = h + p.look[1] * (e - PUPIL / 2);
    k.box(
      pa - PUPIL / 2,
      pa + PUPIL / 2,
      wf - 0.001,
      wf + 0.0015,
      ph - PUPIL / 2,
      ph + PUPIL / 2,
      surf.pupil,
    );
  }
}

const sootPuffs: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { top } = curioHalf(kind, variant);
  const surf = {
    body: s.tinted(SOOT_BLACK),
    fuzz: s.tinted(FUZZ),
    white: s.tinted(EYE_WHITE),
    pupil: s.tinted(PUPIL_BLACK),
  };
  for (const p of PUFFS[variant] ?? []) puff(kitAt, p, top, surf);
  const crumb = s.tinted(CRUMB);
  for (const [a, d, side] of CRUMBS[variant] ?? [])
    k.box(
      a - side / 2,
      a + side / 2,
      d - side / 2,
      d + side / 2,
      0,
      CRUMB_H,
      crumb,
    );
};

/** The critters' recipes. */
export const CRITTER_RECIPES = {
  "hover-drone": hoverDrone,
  "soot-puffs": sootPuffs,
} satisfies Record<
  Extract<CurioKind, "hover-drone" | "soot-puffs">,
  CurioRecipe
>;
