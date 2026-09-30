/**
 * The console room's console (2.6e C8, C24d): a six-sided pedestal, a
 * six-sided desk whose six panels slope down and out from the centre ring
 * to the rim at waist height, and the column on the desk's centre, whose
 * mechanism (the rotor) rises and falls inside it.
 *
 * The console is built centred on the origin, `a` across, `d` front to
 * back and `h` up, in fixed tints (C4) whatever the look:
 *
 * - The pedestal and the desk are one white body: a six-sided post up to
 *   `CONSOLE.pedestalTop`, flaring out underneath the desk to its rim band,
 *   whose top slopes from `CONSOLE.ring` at the centre ring down to
 *   `CONSOLE.rim` at the rim, and is flat inside the ring.
 * - Each of the six facets of that slope carries one grey-green panel
 *   (`CONSOLE_PANEL`), a thin plate lying on the facet from the ring to
 *   the rim, as wide as the facet is at the ring, so a white border
 *   widening towards the rim frames it. A panel and everything on it are
 *   built in the facet's own sheared frame (`facetFrame`): `a` along the
 *   facet, `d` down the slope and `h` straight up, so a box or a cylinder
 *   in it stands upright with its top and foot on the slope.
 * - On each panel: a row of four small lights near the ring on the
 *   `twinkle` bank, two dials, three switches and a lever. The lights take
 *   the room's eight groups in steps no two panels share, so they blink
 *   out of step.
 * - The column's frame: a white base ring on the desk's centre and a white
 *   top ring at `COLUMN.h1`, joined by `COLUMN.ribs` thin pale ribs. There
 *   is no pane: the renderer has no blending, so a pane would hide the
 *   rotor. The ribs and rings read as the column's frame, and the rotor
 *   shows between them.
 *
 * The rotor is not in the console's mesh. It is the room's one mover
 * (C9), built by `rotorMover` in world space at the console's anchor: a
 * rod, three glowing discs and a cage of four slim bars between two
 * plates, resting on the base ring, which the renderer slides up by
 * `ROTOR_TRAVEL` and back over `ROTOR_PERIOD` (`parts.ts`).
 *
 * The sizes are the plan's, with the column and the rotor widened to the
 * research note's measure (C24d). No recipe here sets text (C20).
 */

import type { InteriorPiece } from "../../../world/types";
import { CELL } from "../../../world/units";
import { BLINK_GROUPS } from "../../blink";
import { createBuilder, type V3 } from "../../geometry";
import { createKit, frameAt, type Frame, type Kit } from "../../kit";
import type { Look, Rgb } from "../../looks";
import { surfaces, type Mover, type Surfaces } from "../common";
import { CONSOLE_WALL, type InteriorRecipe } from "./common";

/**
 * The console's sizes (C8), in metres: the desk `corners` across its
 * corners, its `rim` (waist height) and its centre `ring` heights, and the
 * six-sided `pedestal` across its corners, up to `pedestalTop`.
 */
export const CONSOLE = {
  corners: 2.4,
  rim: 0.9,
  ring: 1.0,
  pedestal: 1.0,
  pedestalTop: 0.55,
} as const;

/**
 * The column on the console's centre (C8, C24d): its frame from `h0` (the
 * desk's centre ring) to `h1`, `radius` out, with `ribs` thin vertical
 * ribs and no pane.
 */
export const COLUMN = { h0: 1.0, h1: 2.45, radius: 0.38, ribs: 6 } as const;

/**
 * The rotor at rest (C8, C24d): from `h0` (on the column's base ring) up
 * to `h1`, `radius` out at its widest (its two plates).
 */
export const ROTOR = { h0: 1.1, h1: 2.0, radius: 0.27 } as const;

/**
 * How far the rotor rises at the top of its slide, in metres (C8): from
 * `ROTOR.h1` at rest to 0.15 m under the column's top ring.
 */
export const ROTOR_TRAVEL = 0.3;

/** The console's panels: a grey-green (C8). */
export const CONSOLE_PANEL: Rgb = [0.55, 0.62, 0.58];

/** The column's ribs: a pale grey (C8). */
export const COLUMN_RIB: Rgb = [0.8, 0.85, 0.85];

/** The rotor's discs, glowing through the ribs: a cold near-white (C8). */
export const ROTOR_GLOW: Rgb = [0.85, 0.95, 1.0];

/** The rotor's rod and cage bars, and the panels' switches: silver. */
const SILVER: Rgb = [0.74, 0.77, 0.8];

/** The dials' faces and the lever's slot plate: a dark grey. */
const DIAL: Rgb = [0.2, 0.22, 0.23];

/** The lever's knob: red. */
const KNOB: Rgb = [0.78, 0.16, 0.12];

/**
 * The panel lights' colours, one per light of a row: amber, green, red and
 * white (C8).
 */
const LIGHTS: readonly Rgb[] = [
  [1.0, 0.62, 0.15],
  [0.35, 1.0, 0.45],
  [1.0, 0.22, 0.18],
  [1.0, 1.0, 0.94],
];

/** The desk's six sides. */
const SIDES = 6;

/**
 * The corner radius of the desk's centre ring, in metres: where the six
 * facets start sloping down. The column's base ring sits inside it.
 */
const RING_RADIUS = 0.5;

/** How far under the rim the desk's rim band reaches, in metres. */
const RIM_BAND = 0.08;

/** How far a panel stands proud of its facet, in metres. */
const PANEL_PROUD = 0.003;

/**
 * How much narrower than its facet is at the ring a panel is, in metres,
 * so a white line keeps the panels apart there.
 */
const PANEL_MARGIN = 0.06;

/** The column's rings: their radius and each ring's height, in metres. */
const COLUMN_RING = { radius: 0.4, base: 0.1, top: 0.06, sides: 18 } as const;

/**
 * A rib's side, in metres, and how far out from the column's axis it stands.
 */
const RIB = { side: 0.02, at: 0.37 } as const;

/** Facets round the column's rings, the dials and the rotor's round parts. */
const ROUND = 16;

/**
 * The apothem of a regular hexagon of corner radius `r`: its centre to a
 * side's middle.
 */
const apothem = (r: number) => r * Math.cos(Math.PI / SIDES);

/**
 * A facet's slope down and out, in metres of drop per metre along the
 * slope's plan.
 */
const SLOPE =
  (CONSOLE.ring - CONSOLE.rim) /
  (apothem(CONSOLE.corners / 2) - apothem(RING_RADIUS));

/** How far a facet runs in plan from the ring to the rim, in metres. */
const FACET_RUN = apothem(CONSOLE.corners / 2) - apothem(RING_RADIUS);

/**
 * Facet `i`'s own frame, sheared to its slope: its origin on the ring at
 * the middle of the facet's inner edge, at `CONSOLE.ring`; `a` along the
 * facet; `d` out from the ring down the slope, so a point at `(a, d, 0)`
 * lies on the facet; and `h` straight up. A lathe of `SIDES` facets puts
 * its first corner on `+d` of `f`, so facet `i`'s middle is at
 * `(i + 1/2) / SIDES` of a turn from there. The shear only adds a multiple
 * of up to `inward`, which keeps every primitive's winding.
 */
function facetFrame(f: Frame, i: number): Frame {
  const t = ((i + 0.5) * 2 * Math.PI) / SIDES;
  const u: V3 = [
    f.along[0] * Math.sin(t) + f.inward[0] * Math.cos(t),
    0,
    f.along[2] * Math.sin(t) + f.inward[2] * Math.cos(t),
  ];
  const r = apothem(RING_RADIUS);
  return {
    origin: [
      f.origin[0] + u[0] * r,
      f.origin[1] + CONSOLE.ring,
      f.origin[2] + u[2] * r,
    ],
    along: [u[2], 0, -u[0]],
    inward: [u[0], -SLOPE, u[2]],
  };
}

/**
 * The blink group of light `j` on panel `i`: strides of 5 panels and 3
 * lights round the `twinkle` bank's eight groups, so every group is used,
 * no two lights of a row share one and no two panels light in the same
 * order.
 */
const lightGroup = (i: number, j: number) => (5 * i + 3 * j) % BLINK_GROUPS;

/**
 * Panel `i` and what stands on it, in the facet's frame (`facetFrame`):
 * the plate, a row of four lights near the ring, two dials, three
 * switches and a lever with its slot plate and knob. Everything keeps
 * within 0.15 m of the panel's middle line, clear of the next panels.
 */
function panel(k: Kit, s: Surfaces, i: number): void {
  const half = RING_RADIUS / 2 - PANEL_MARGIN / 2;
  const top = PANEL_PROUD;
  k.box(-half, half, 0, FACET_RUN, 0, top, s.tinted(CONSOLE_PANEL));
  LIGHTS.forEach((tint, j) => {
    const a = (j - 1.5) * 0.07;
    k.cylinder(
      a,
      0.1,
      top,
      top + 0.015,
      0.018,
      8,
      s.blink(tint, lightGroup(i, j)),
    );
  });
  for (const a of [-0.09, 0.09])
    k.cylinder(a, 0.28, top, top + 0.02, 0.05, ROUND, s.tinted(DIAL));
  const silver = s.tinted(SILVER);
  for (const a of [-0.12, -0.06, 0])
    k.box(a - 0.0125, a + 0.0125, 0.44, 0.49, top, top + 0.03, silver);
  k.box(0.085, 0.135, 0.42, 0.52, top, top + 0.01, s.tinted(DIAL));
  k.box(0.104, 0.116, 0.45, 0.462, top + 0.01, top + 0.11, silver);
  k.box(0.1, 0.12, 0.446, 0.466, top + 0.11, top + 0.14, s.tinted(KNOB));
}

/**
 * The console (C8): the white pedestal and desk, the six sloping panels
 * with their switches, dials, lever and lights, and the column's open
 * frame of two rings and six ribs.
 */
const consoleRecipe: InteriorRecipe = ({ k, kitAt, s }) => {
  const white = s.tinted(CONSOLE_WALL);
  const foot = CONSOLE.pedestal / 2;
  const rim = CONSOLE.corners / 2;
  k.cylinder(0, 0, 0, CONSOLE.pedestalTop, foot, SIDES, white);
  k.lathe(
    0,
    0,
    [
      [foot, CONSOLE.pedestalTop],
      [rim, CONSOLE.rim - RIM_BAND],
      [rim, CONSOLE.rim],
      [RING_RADIUS, CONSOLE.ring],
      [0, CONSOLE.ring],
    ],
    SIDES,
    white,
  );
  const base = frameAt([0, 0, 0], 0);
  for (let i = 0; i < SIDES; i++) panel(kitAt(facetFrame(base, i)), s, i);

  const ringLow = COLUMN.h0 + COLUMN_RING.base;
  const ringHigh = COLUMN.h1 - COLUMN_RING.top;
  const R = COLUMN_RING;
  k.cylinder(0, 0, COLUMN.h0, ringLow, R.radius, R.sides, white);
  const rib = s.tinted(COLUMN_RIB);
  for (let i = 0; i < COLUMN.ribs; i++) {
    const t = (i * 2 * Math.PI) / COLUMN.ribs;
    const a = RIB.at * Math.sin(t);
    const d = RIB.at * Math.cos(t);
    const h = RIB.side / 2;
    k.box(a - h, a + h, d - h, d + h, ringLow, ringHigh, rib);
  }
  k.cylinder(0, 0, ringHigh, COLUMN.h1, R.radius, R.sides, white);
};

/** The console's recipe, for the family's table in `index.ts`. */
export const CONSOLE_RECIPES = {
  console: consoleRecipe,
} satisfies Record<"console", InteriorRecipe>;

/**
 * The rotor at rest in `k`'s frame, centred on its origin: a white plate
 * at each end `ROTOR.radius` out, a silver rod up the middle, three
 * glowing discs stacked on it (`s.signal(ROTOR_GLOW)`: a mover draws in
 * the steady bank, so the discs glow without blinking) and a cage of four
 * slim silver bars round them. `rotorMover` builds it at the console's
 * anchor; the family's test records it through a recording kit.
 */
export function buildRotor(k: Kit, s: Surfaces): void {
  const { h0, h1, radius } = ROTOR;
  const plate = 0.03;
  const white = s.tinted(CONSOLE_WALL);
  const silver = s.tinted(SILVER);
  k.cylinder(0, 0, h0, h0 + plate, radius, ROUND, white);
  k.cylinder(0, 0, h1 - plate, h1, radius, ROUND, white);
  k.cylinder(0, 0, h0 + plate, h1 - plate, 0.03, 8, silver);
  const glow = s.signal(ROTOR_GLOW);
  const span = h1 - h0 - 2 * plate;
  for (const f of [0.25, 0.5, 0.75]) {
    const h = h0 + plate + span * f;
    k.cylinder(0, 0, h - 0.02, h + 0.02, 0.2, ROUND, glow);
  }
  const bar = 0.01;
  const at = 0.24;
  for (let i = 0; i < 4; i++) {
    const t = ((i + 0.5) * Math.PI) / 2;
    const a = at * Math.sin(t);
    const d = at * Math.cos(t);
    k.box(a - bar, a + bar, d - bar, d + bar, h0 + plate, h1 - plate, silver);
  }
}

/**
 * The rotor of the console at `index` in `room.interior` (C8, C9, C17):
 * its mesh built in world space at the piece's anchor and turn, at rest,
 * keyed `rotor:<index>`. No fixture's fault frame names it (`fixture`
 * -1); it slides straight up by `ROTOR_TRAVEL` and back on the clock alone
 * (`rotorPhase`), with no pivot and no turn, at full gain.
 */
export function rotorMover(
  piece: InteriorPiece,
  index: number,
  look: Look,
): Mover {
  const b = createBuilder();
  const k = createKit(
    b,
    frameAt([piece.x * CELL, 0, piece.y * CELL], piece.turn),
  );
  buildRotor(k, surfaces(look));
  return {
    key: `rotor:${String(index)}`,
    part: "rotor",
    fixture: -1,
    mesh: b.build(),
    axis: [0, 1, 0],
    travel: ROTOR_TRAVEL,
    pivot: null,
    rest: 1,
    swing: 0,
  };
}
