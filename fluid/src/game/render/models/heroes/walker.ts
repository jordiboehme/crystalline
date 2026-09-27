/**
 * The slab walker's recipe (2.6d C17): a robot of four tall slabs of
 * brushed grey metal side by side, caught mid-stride. Colours and helpers
 * stay in this file, which imports only `common.ts` of the hero batches.
 *
 * - The four slabs run along `a`, each `WALKER.slab` wide and
 *   `WALKER.depth` deep, `WALKER.gap` apart, so the four span the
 *   footprint's 0.9 m. Each is its own shade of grey (`SLAB_GREYS`), so
 *   they read as four blocks and not as one.
 * - The two inner slabs stand upright from the floor to `WALKER.height`:
 *   a bevelled body, a dark recessed joint band `JOINT` tall and a
 *   bevelled head block over it.
 * - The two outer slabs, `WALKER.outer` long, hang from hinges at
 *   `WALKER.pivot` on the centre line (`d` 0) and swing `WALKER.swing`
 *   about the `a` axis, the one at `-a` with its foot forward (`+d`) and
 *   the one at `+a` with its foot back, both feet clear of the floor as if
 *   mid-step. The kit has no pitch, so each is built in the side view:
 *   `profileAlong` of its outline, chamfered at its ends like the inner
 *   slabs' bevel, in the same three pieces (body, joint, head).
 * - Dark hinge pins (`cylinderAlong`) on the hinge line join each outer
 *   slab to its inner neighbour and the two inner slabs to each other,
 *   so the swung slabs hang from them.
 * - Four fine panel lines cross every slab, front and back, each one
 *   bar 3 mm proud of both faces and inset from the slab's sides, so no
 *   face lies in another's plane.
 * - One small indicator on the front of the inner slab at `+a`, just
 *   under its joint, is the only light: a soft white-blue `s.signal`. The
 *   bank is `steady`. No display strip and no text.
 */

import type { HeroKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { frameAt, type Frame, type Kit } from "../../kit";
import { LAYER } from "../../layers";
import type { Rgb } from "../../looks";
import { profileAlong, type KitAt } from "../common";
import type { HeroRecipe } from "./common";

/** The walker is built in this frame, at the origin. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/**
 * The walker's sizes, in metres and radians (C17): each slab's width
 * along `a`, the gap between two slabs, the slabs' depth along `d`, the
 * inner slabs' height, the outer slabs' length, the hinge height and the
 * outer slabs' swing from upright.
 */
export const WALKER = {
  slab: 0.2,
  gap: 1 / 30,
  depth: 0.25,
  height: 1.8,
  outer: 1.75,
  pivot: 1.4,
  swing: (25 * Math.PI) / 180,
} as const;

/** How far an outer slab reaches above its hinge, along its own axis. */
const ABOVE = WALKER.outer - WALKER.pivot;

/** The four slabs' greys, from `-a` to `+a`: each its own shade of brushed metal. */
const SLAB_GREYS: readonly Rgb[] = [
  [0.55, 0.56, 0.58],
  [0.5, 0.51, 0.53],
  [0.58, 0.59, 0.61],
  [0.52, 0.53, 0.55],
];

/** The panel lines and the recessed joints: a darker grey. */
const LINE_GREY: Rgb = [0.33, 0.34, 0.36];

/** The hinge pins: near black. */
const HINGE_DARK: Rgb = [0.2, 0.2, 0.22];

/** The indicator light: a soft white-blue. */
const INDICATOR: Rgb = [0.7, 0.85, 1.0];

/** The inner slabs' bevel, and the chamfer at the outer slabs' ends. */
const BEVEL = 0.01;

/** The joint band's height, and how far its top lies under a slab's top. */
const JOINT = 0.02;
const JOINT_UNDER = 0.13;

/** How far the joint band is recessed into the slab, on every side. */
const JOINT_INSET = 0.004;

/** The panel lines' heights on an inner slab, from the floor. */
const LINES = [0.35, 0.7, 1.05, 1.5] as const;

/** A panel line's height, how far it stands proud and how far it is inset from the slab's sides. */
const LINE_H = 0.006;
const LINE_PROUD = 0.003;
const LINE_INSET = 0.004;

/** The hinge pins' radius and facets, and how far a pin reaches into a slab. */
const PIN_R = 0.03;
const PIN_SIDES = 10;
const PIN_INTO = 0.01;

/** The indicator's side and its bottom, on the front of the inner slab at `+a`. */
const LIGHT = 0.012;
const LIGHT_H = 1.62;

/** A slab's centre along `a`: `i` 0 to 3 from `-a` to `+a`. */
const slabCentre = (i: number) => (i - 1.5) * (WALKER.slab + WALKER.gap);

/**
 * An inner slab centred at `a`: a bevelled body from the floor to the
 * joint, a recessed dark joint band, a bevelled head to the top, and the
 * four panel lines, each a thin dark bar through the body, proud of its
 * front and back faces.
 */
function innerSlab(k: Kit, a: number, body: Surface, line: Surface): void {
  const W = WALKER;
  const [a0, a1] = [a - W.slab / 2, a + W.slab / 2];
  const hd = W.depth / 2;
  const j1 = W.height - JOINT_UNDER;
  const j0 = j1 - JOINT;
  k.bevelBox(a0, a1, -hd, hd, 0, j0, BEVEL, body);
  k.box(
    a0 + JOINT_INSET,
    a1 - JOINT_INSET,
    -hd + JOINT_INSET,
    hd - JOINT_INSET,
    j0 - 0.002,
    j1 + 0.002,
    line,
  );
  k.bevelBox(a0, a1, -hd, hd, j1, W.height, BEVEL, body);
  for (const h of LINES)
    k.box(
      a0 + LINE_INSET,
      a1 - LINE_INSET,
      -hd - LINE_PROUD,
      hd + LINE_PROUD,
      h - LINE_H / 2,
      h + LINE_H / 2,
      line,
    );
}

/**
 * An outer slab centred at `a`, hanging from its hinge at `(d 0, h
 * pivot)` and swung `s * swing` from upright: `s` +1 leans its top back
 * and swings its foot forward to `+d`, -1 the other way. `at(t, n)` is
 * the side-view point `t` along the slab's axis from the hinge (up
 * positive) and `n` across it; every piece is an outline in those terms,
 * so the body, the joint, the head and the lines all swing together.
 */
function outerSlab(
  kitAt: KitAt,
  a: number,
  s: 1 | -1,
  body: Surface,
  line: Surface,
): void {
  const W = WALKER;
  const axis = [-s * Math.sin(W.swing), Math.cos(W.swing)] as const;
  const across = [axis[1], -axis[0]] as const;
  const at = (t: number, n: number): [number, number] => [
    axis[0] * t + across[0] * n,
    W.pivot + axis[1] * t + across[1] * n,
  ];
  const hd = W.depth / 2;
  const c = BEVEL;
  const foot = -W.pivot;
  const j1 = ABOVE - JOINT_UNDER;
  const j0 = j1 - JOINT;
  const [a0, a1] = [a - W.slab / 2, a + W.slab / 2];
  const piece = (
    outline: [number, number][],
    b0: number,
    b1: number,
    sf: Surface,
  ) => profileAlong(kitAt, ORIGIN, outline, b0, b1, sf);
  // The body, chamfered at the foot.
  piece(
    [
      at(foot, -hd + c),
      at(foot + c, -hd),
      at(j0, -hd),
      at(j0, hd),
      at(foot + c, hd),
      at(foot, hd - c),
    ],
    a0,
    a1,
    body,
  );
  // The recessed joint band, reaching a little into the body and the head.
  const ji = hd - JOINT_INSET;
  piece(
    [
      at(j0 - 0.002, -ji),
      at(j1 + 0.002, -ji),
      at(j1 + 0.002, ji),
      at(j0 - 0.002, ji),
    ],
    a0 + JOINT_INSET,
    a1 - JOINT_INSET,
    line,
  );
  // The head, chamfered at the top.
  piece(
    [
      at(j1, -hd),
      at(ABOVE - c, -hd),
      at(ABOVE, -hd + c),
      at(ABOVE, hd - c),
      at(ABOVE - c, hd),
      at(j1, hd),
    ],
    a0,
    a1,
    body,
  );
  // The panel lines at the inner slabs' distances under the top.
  const lp = hd + LINE_PROUD;
  for (const h of LINES) {
    const t = ABOVE - (W.height - h);
    piece(
      [
        at(t - LINE_H / 2, -lp),
        at(t + LINE_H / 2, -lp),
        at(t + LINE_H / 2, lp),
        at(t - LINE_H / 2, lp),
      ],
      a0 + LINE_INSET,
      a1 - LINE_INSET,
      line,
    );
  }
}

const slabWalker: HeroRecipe = ({ k, kitAt, s }) => {
  const W = WALKER;
  const line = s.tinted(LINE_GREY);
  const hinge = s.tinted(HINGE_DARK);
  const grey = (i: number) => s.tinted(SLAB_GREYS[i] ?? LINE_GREY, LAYER.metal);
  innerSlab(k, slabCentre(1), grey(1), line);
  innerSlab(k, slabCentre(2), grey(2), line);
  outerSlab(kitAt, slabCentre(0), 1, grey(0), line);
  outerSlab(kitAt, slabCentre(3), -1, grey(3), line);
  // The hinge pins: across each outer gap into the swung slab, and
  // across the middle gap between the inner two.
  const innerEdge = slabCentre(2) + W.slab / 2;
  const outerEdge = slabCentre(3) - W.slab / 2;
  for (const sign of [-1, 1])
    k.cylinderAlong(
      sign * innerEdge,
      sign * (outerEdge + PIN_INTO),
      0,
      W.pivot,
      PIN_R,
      PIN_SIDES,
      hinge,
    );
  const middle = W.gap / 2 + PIN_INTO;
  k.cylinderAlong(-middle, middle, 0, W.pivot, PIN_R, PIN_SIDES, hinge);
  // The indicator, on the front face of the inner slab at +a.
  const la = slabCentre(2);
  const front = W.depth / 2;
  k.box(
    la - LIGHT / 2,
    la + LIGHT / 2,
    front - 0.002,
    front + 0.002,
    LIGHT_H,
    LIGHT_H + LIGHT,
    s.signal(INDICATOR),
  );
};

/** The slab walker's recipe. */
export const WALKER_RECIPES = {
  "slab-walker": slabWalker,
} satisfies Record<Extract<HeroKind, "slab-walker">, HeroRecipe>;
