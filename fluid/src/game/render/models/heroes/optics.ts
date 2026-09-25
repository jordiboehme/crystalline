/**
 * The optics heroes' recipes: the turret, the black slab, the eye panel,
 * the photo console and the laser desk. What they share is a lens or a
 * screen: a glowing eye or lens set in a housing (the turret, the eye
 * panel, the laser desk's emitter), a screen bank on a desk (the photo
 * console, the laser desk's terminal), and the slab, which is all housing
 * and no light.
 *
 * This file sets the hero style the other batches follow:
 * - Sizes come from `heroHalf`; the heights a kind is built to are the
 *   plan's, named in a table at the top of each recipe.
 * - Housings are bevelled boxes (`HOUSING_BEVEL`, a thin chamfer that
 *   catches the light), except where a crisp edge is the point (the slab).
 * - Round parts use few facets (10 to 12 around a body, 4 around a thin
 *   tube), so every hero stays well under 1500 triangles.
 * - A lens is always `lens`: a dark bezel ring with a glowing disc set just
 *   in front of its middle, so every eye in the station is built the same
 *   way and blinks with its kind's bank.
 * - Colours are the look's surfaces, or the named tints below for what no
 *   palette carries (a white shell, a red lens, a screen's own hue).
 */

import type { HeroKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import {
  profileAlong,
  tiltedBar,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import { heroHalf, type HeroRecipe } from "./common";

/** The turret's shell: a clean glossy white, the same in every look. */
const WHITE_SHELL: Rgb = [0.93, 0.93, 0.9];

/** Every lens's light: a deep warning red. */
const LENS_RED: Rgb = [1.0, 0.1, 0.06];

/** The slab's finish: near black, matte (lit, so it still takes a faint edge). */
const SLAB_BLACK: Rgb = [0.02, 0.02, 0.025];

/** The photo console's screen: a sepia grey, an old print under glass. */
const SEPIA_GREY: Rgb = [0.62, 0.56, 0.46];

/** The laser desk's monitor: a dark cyan glow, a terminal late at night. */
const DARK_CYAN: Rgb = [0.1, 0.42, 0.48];

/** A status light that says all is well. */
const STATUS_GREEN: Rgb = [0.25, 1.0, 0.35];

/** A status light that says wait. */
const STATUS_AMBER: Rgb = [1.0, 0.6, 0.12];

/** The office chair's cushions: a charcoal fabric. */
const CHAIR_FABRIC: Rgb = [0.16, 0.16, 0.18];

/** The chamfer of a hero's housings, in metres. */
const HOUSING_BEVEL = 0.02;

/** A lens bezel's tube radius as a share of the lens's radius. */
const BEZEL_SHARE = 0.25;

/** The thinnest a lens bezel's tube gets, in metres. */
const BEZEL_MIN = 0.012;

/** Facets around a lens: its bezel ring's segments and its disc's sides. */
const LENS_SIDES = 12;

/** A regular `n`-gon of radius `r` around `(a, h)`: an outline for `extrude`. */
function roundel(
  a: number,
  h: number,
  r: number,
  n: number,
): [number, number][] {
  return Array.from({ length: n }, (_, i) => {
    const t = (2 * Math.PI * i) / n;
    return [a + r * Math.cos(t), h + r * Math.sin(t)] as [number, number];
  });
}

/** Which way a lens looks: out of the frame's wall plane, up or down. */
export type LensFacing = "front" | "up" | "down";

/**
 * A lens: a dark bezel ring of tube `max(BEZEL_MIN, radius * BEZEL_SHARE)`
 * whose hole is exactly `radius`, centred at `(a, d, h)`, with a glowing
 * disc of `radius` in it, drawn with `s.blink(tint, group)` so it pulses
 * with its hero's blink bank. The disc stands from the bezel's back to
 * half a tube in front of the bezel's middle plane, so it sits recessed in
 * the ring and touches it all round (the glow check's host).
 *
 * `facing` `"front"` (the default) stands the ring in the frame's wall
 * plane, looking along `+d`; the disc is a thin extruded `LENS_SIDES`-gon.
 * A `Frame` is always upright, so no sub-frame can look up or down:
 * `"up"` and `"down"` lay the ring flat instead and make the disc a thin
 * upright cylinder, standing out of the ring on the side it looks to.
 */
export function lens(
  k: Kit,
  s: Surfaces,
  a: number,
  d: number,
  h: number,
  radius: number,
  tint: Rgb,
  group: number,
  facing: LensFacing = "front",
): void {
  const tube = Math.max(BEZEL_MIN, radius * BEZEL_SHARE);
  const light = s.blink(tint, group);
  if (facing === "front") {
    k.ring(a, d, h, radius + tube, tube, 4, LENS_SIDES, s.dark, "inward");
    k.extrude(roundel(a, h, radius, LENS_SIDES), d - tube, d + tube / 2, light);
    return;
  }
  const out = facing === "up" ? 1 : -1;
  k.ring(a, d, h, radius + tube, tube, 4, LENS_SIDES, s.dark, "up");
  k.cylinder(
    a,
    d,
    h - out * tube,
    h + (out * tube) / 2,
    radius,
    LENS_SIDES,
    light,
  );
}

/**
 * The office chair's heights: its seat cushion (`seat`, bottom to top),
 * the back cushion and the reach of the five-star base's arms, in metres.
 * Exported so the laser desk's test can find the seat among the parts.
 */
export const OFFICE_CHAIR = {
  seat: [0.42, 0.5],
  seatHalf: 0.23,
  back: [0.58, 0.98],
  arm: 0.28,
} as const;

/**
 * An office chair centred at `(a, d)` of the frame `f`: a five-star base
 * on casters, a gas column, a square seat cushion and a back cushion on a
 * post, the charcoal cushions on dark metal. It faces `+d` of `f` at
 * `turn` 0 and is turned about its column by `turn` quarter turns the way
 * `yawed` turns (each quarter swings its front from `+d` towards `-a`), so
 * 2 faces `-d`. It takes `kitAt` and a frame, not a kit: the base's arms
 * run at fifths of a turn, which only a yawed sub-frame can build.
 */
export function officeChair(
  kitAt: KitAt,
  f: Frame,
  s: Surfaces,
  a: number,
  d: number,
  turn: number,
): void {
  const C = OFFICE_CHAIR;
  const fc = yawed(f, a, d, (turn * Math.PI) / 2);
  const k = kitAt(fc);
  const fabric = s.tinted(CHAIR_FABRIC);
  for (let i = 0; i < 5; i++) {
    const arm = kitAt(yawed(fc, 0, 0, (2 * Math.PI * i) / 5));
    arm.box(-0.02, 0.02, 0.03, C.arm, 0.04, 0.08, s.dark);
    arm.cylinder(0, C.arm - 0.02, 0, 0.04, 0.025, 6, s.dark);
  }
  k.cylinder(0, 0, 0.03, 0.09, 0.05, 8, s.dark);
  k.cylinder(0, 0, 0.09, C.seat[0], 0.025, 8, s.metal);
  const w = C.seatHalf;
  k.bevelBox(-w, w, -w, w, C.seat[0], C.seat[1], 0.03, fabric);
  k.box(
    -0.03,
    0.03,
    -w + 0.01,
    -w + 0.06,
    C.seat[1],
    C.back[0] + 0.04,
    s.metal,
  );
  k.bevelBox(
    -0.2,
    0.2,
    -w - 0.01,
    -w + 0.05,
    C.back[0],
    C.back[1],
    0.025,
    fabric,
  );
}

/**
 * The turret: a white egg on three thin splayed legs, with one red eye.
 * The legs run from a dark hub under the egg (h 0.55) out to near the
 * box's edge on the floor, 120 degrees apart with one pointing back
 * (`-d`), each a `tiltedBar` in a yawed frame's side view on a small foot
 * pad. The egg is a 10-sided lathe from h 0.5 to 1.3, widest (r 0.3) at
 * h 0.85, with a dark seam ring at h 0.92. The eye sits in a dark socket
 * on the front at h 1.02: a red `lens` of r 0.06, blink group 0, which
 * the breathe bank pulses slowly.
 */
const turret: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { d1 } = heroHalf(kind, variant);
  const f = frameAt([0, 0, 0], 0);
  const shell = s.tinted(WHITE_SHELL);
  // Legs: hub end inside the hub, foot end just off the floor.
  const [d0, h0, dF, hF] = [0.03, 0.56, d1 - 0.06, 0.03];
  const leg = tiltedBar(
    (d0 + dF) / 2,
    (h0 + hF) / 2,
    Math.atan2(hF - h0, dF - d0),
    Math.hypot(dF - d0, hF - h0),
    0.035,
  );
  for (const yaw of [Math.PI, Math.PI / 3, -Math.PI / 3]) {
    const fl = yawed(f, 0, 0, yaw);
    profileAlong(kitAt, fl, leg, -0.0175, 0.0175, s.dark);
    kitAt(fl).cylinder(0, dF, 0, 0.03, 0.045, 8, s.dark);
  }
  k.cylinder(0, 0, 0.48, 0.6, 0.07, 10, s.dark);
  k.lathe(
    0,
    0,
    [
      [0, 0.5],
      [0.14, 0.53],
      [0.22, 0.6],
      [0.275, 0.7],
      [0.3, 0.85],
      [0.285, 0.98],
      [0.25, 1.08],
      [0.19, 1.17],
      [0.11, 1.25],
      [0, 1.3],
    ],
    10,
    shell,
  );
  k.ring(0, 0, 0.92, 0.29, 0.012, 4, 20, s.dark, "up");
  // The eye socket stands out of the shell, which leans back above the seam.
  const eyeH = 1.02;
  const eyeD = 0.3;
  k.extrude(roundel(0, eyeH, 0.09, LENS_SIDES), 0.12, eyeD, s.dark);
  lens(k, s, 0, eyeD, eyeH, 0.06, LENS_RED, 0);
};

/**
 * The black slab: one plain box over the whole footprint from the floor
 * to its top, 1 : 4 : 9 (0.3 by 1.2 by 2.7 m), near black and lit. No
 * bevel and no light: its crisp edges are the whole of it.
 */
const blackSlab: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw, d0, d1, top } = heroHalf(kind, variant);
  k.box(-hw, hw, d0, d1, 0, top, s.tinted(SLAB_BLACK));
};

/**
 * The eye panel, flush on its wall: a tall brushed-metal panel (d 0 to
 * 0.08, h 0.4 to the top, the width less 0.05 each side) with a dark inset
 * face, a round metal lens housing standing out to d 0.2 at h 1.35 with a
 * red `lens` (r 0.09, blink group 0) in it, a speaker grille of six thin
 * bars from h 0.8 to 1.1, a blank name plate at h 1.75 and four bolts.
 */
const eyePanel: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw, top } = heroHalf(kind, variant);
  const pw = hw - 0.05;
  const face = 0.09;
  k.bevelBox(-pw, pw, 0, 0.08, 0.4, top, HOUSING_BEVEL, s.metal);
  k.box(-pw + 0.06, pw - 0.06, 0.08, face, 0.5, top - 0.1, s.dark);
  for (const a of [-pw + 0.03, pw - 0.03])
    for (const h of [0.45, top - 0.05])
      k.box(a - 0.015, a + 0.015, 0.08, face, h - 0.015, h + 0.015, s.dark);
  // The housing: a short metal barrel with a rim ring whose front is at 0.2.
  const eyeH = 1.35;
  const rimD = 0.17;
  k.extrude(roundel(0, eyeH, 0.15, 16), face, rimD, s.metal);
  k.ring(0, rimD, eyeH, 0.13, 0.03, 4, 16, s.metal, "inward");
  lens(k, s, 0, rimD, eyeH, 0.09, LENS_RED, 0);
  for (let i = 0; i < 6; i++) {
    const h = 0.8 + i * 0.055;
    k.box(-0.2, 0.2, face, face + 0.015, h, h + 0.025, s.metal);
  }
  k.box(-0.22, 0.22, face, face + 0.01, 1.7, 1.8, s.panel);
};

/** The photo console's sloped deck: its top at depth `d`, from 0.95 at d 0.45 to 0.75 at d 0.9. */
const deckTop = (d: number) => 0.95 - ((d - 0.45) * 0.2) / 0.45;

/** How far a small status light stands out of its face, in metres. */
const LIGHT_DEPTH = 0.01;

/** The six status lights along the photo console's front edge, one per blink group. */
const CONSOLE_LIGHTS: readonly Rgb[] = [
  STATUS_GREEN,
  STATUS_GREEN,
  STATUS_AMBER,
  STATUS_GREEN,
  LENS_RED,
  STATUS_AMBER,
];

/**
 * The photo console, backed against its wall: a tall tower (d 0 to 0.4,
 * up to the top) holding a large square screen facing `+d` (h 1.0 to 1.7,
 * a dark bezel, a sepia-grey `s.glow` screen, a visor above and two rows
 * of buttons beside it), and in front of it a console desk whose deck
 * slopes from 0.95 at d 0.45 down to 0.75 at the front edge (d 0.9),
 * carrying a keypad (a grid of small keys following the slope), a dark
 * print tray and a trackball. Six status lights along the deck's front
 * edge blink in groups 0 to 5 of the status bank.
 */
const photoConsole: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, d1, top } = heroHalf(kind, variant);
  const f = frameAt([0, 0, 0], 0);
  // The desk's front face stands a light's depth behind the footprint's
  // edge, so the status lights on it end exactly on that edge.
  const front = d1 - LIGHT_DEPTH;
  const tw = 0.6;
  const tower = 0.4;
  k.bevelBox(-tw, tw, 0, tower, 0, top, HOUSING_BEVEL * 1.5, s.body);
  // The desk, with a toe recess under its front edge.
  const dw = hw - 0.02;
  profileAlong(
    kitAt,
    f,
    [
      [tower, 0],
      [front - 0.06, 0],
      [front - 0.06, 0.1],
      [front, 0.1],
      [front, deckTop(front)],
      [0.45, deckTop(0.45)],
      [tower, deckTop(0.45)],
    ],
    -dw,
    dw,
    s.body,
  );
  // The screen.
  k.box(-0.42, 0.42, tower, tower + 0.04, 0.96, 1.74, s.dark);
  k.panel(-0.35, 0.35, tower + 0.04 + DECAL_LIFT, 1.0, 1.7, s.glow(SEPIA_GREY));
  k.bevelBox(-0.45, 0.45, tower, tower + 0.1, 1.76, 1.84, 0.01, s.body);
  for (const a of [-0.515, 0.515])
    for (const h of [1.2, 1.35, 1.5])
      k.box(a - 0.03, a + 0.03, tower, tower + 0.015, h, h + 0.04, s.dark);
  // On the deck: keys and the tray follow the slope, as thin sloped slabs.
  const slab = (
    a0: number,
    a1: number,
    e0: number,
    e1: number,
    below: number,
    above: number,
    sf: Surface,
  ) =>
    profileAlong(
      kitAt,
      f,
      [
        [e0, deckTop(e0) - below],
        [e1, deckTop(e1) - below],
        [e1, deckTop(e1) + above],
        [e0, deckTop(e0) + above],
      ],
      a0,
      a1,
      sf,
    );
  for (let r = 0; r < 3; r++)
    for (let c = 0; c < 4; c++) {
      const e0 = 0.55 + r * 0.09;
      const a0 = -0.7 + c * 0.1;
      slab(a0, a0 + 0.07, e0, e0 + 0.065, 0.005, 0.015, s.dark);
    }
  slab(-0.2, 0.25, 0.52, 0.82, 0.01, 0.01, s.dark);
  // The trackball in its socket.
  const [ta, td] = [0.55, 0.66];
  const th = deckTop(td);
  k.cylinder(ta, td, th - 0.04, th + 0.01, 0.065, 10, s.dark);
  k.lathe(
    ta,
    td,
    [
      [0, th],
      [0.045, th],
      [0.05, th + 0.02],
      [0.045, th + 0.04],
      [0.028, th + 0.055],
      [0, th + 0.06],
    ],
    10,
    s.panel,
  );
  CONSOLE_LIGHTS.forEach((tint, i) => {
    const a = -0.5 + i * 0.2;
    k.box(a - 0.025, a + 0.025, front, d1, 0.66, 0.7, s.blink(tint, i));
  });
};

/**
 * The laser desk, standing free: `d` from -1.5 to 1.5, its front at `+d`.
 * - A terminal desk: a top slab (h 0.70 to 0.74) over `a` -1.2 to 1.2 and
 *   `d` -0.3 to 0.5 on four legs, a boxy monitor on its right (`a` 0.1 to
 *   0.7, h 0.74 to 1.2) with a dark cyan `s.glow` screen facing `+d`, and a
 *   keyboard in front of it (`d` 0.2 to 0.4). The desk top from `a` -1.1
 *   to -0.35 stays clear: it is the catalogue's surface.
 * - An empty office chair at `a` 0.3, `d` 1.0, facing the desk.
 * - The gantry: a heavy column on a bevelled foot plate at `a` -0.9, `d`
 *   -1.3, up to 2.1, carrying three status lights; an articulated arm
 *   (h 1.95 to 2.1) from a shoulder drum on the column forward to an elbow
 *   drum at `d` 0, then on a diagonal forearm to a wrist drum right over
 *   the chair; from the wrist the emitter hangs: a vertical barrel (r 0.12,
 *   h 1.35 to 1.95) with four cooling fins and a red lens at its foot
 *   looking down at the seat, and a thin beam guide rod (r 0.01) down its
 *   side from h 1.9 to 1.3.
 * Lights: the lens is blink group 0, the beam guide group 1 and the column
 * lights groups 2 to 4, all in the breathe bank.
 */
const laserDesk: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, top } = heroHalf(kind, variant);
  const f = frameAt([0, 0, 0], 0);
  // The desk.
  const [deskD0, deskD1, deskH] = [-0.3, 0.5, 0.74];
  k.bevelBox(-hw, hw, deskD0, deskD1, 0.7, deskH, 0.01, s.body);
  for (const a of [-hw + 0.05, hw - 0.05])
    for (const d of [deskD0 + 0.05, deskD1 - 0.05])
      k.box(a - 0.025, a + 0.025, d - 0.025, d + 0.025, 0, 0.7, s.metal);
  k.bevelBox(0.1, 0.7, -0.25, 0.15, deskH, 1.2, HOUSING_BEVEL, s.body);
  k.box(0.13, 0.67, 0.15, 0.16, 0.8, 1.16, s.dark);
  k.panel(0.16, 0.64, 0.16 + DECAL_LIFT, 0.83, 1.13, s.glow(DARK_CYAN));
  k.box(0.15, 0.65, 0.2, 0.4, deskH, deskH + 0.025, s.dark);
  // The chair, pulled up to the desk.
  const [chairA, chairD] = [0.3, 1.0];
  officeChair(kitAt, f, s, chairA, chairD, 2);
  // The column.
  const [colA, colD] = [-0.9, -1.3];
  k.bevelBox(
    colA - 0.25,
    colA + 0.25,
    colD - 0.18,
    colD + 0.18,
    0,
    0.06,
    HOUSING_BEVEL,
    s.dark,
  );
  k.bevelBox(
    colA - 0.1,
    colA + 0.1,
    colD - 0.1,
    colD + 0.1,
    0.06,
    2.1,
    HOUSING_BEVEL,
    s.body,
  );
  [STATUS_GREEN, STATUS_AMBER, LENS_RED].forEach((tint, i) => {
    const h = 1.2 + i * 0.12;
    k.box(
      colA - 0.04,
      colA + 0.04,
      colD + 0.1,
      colD + 0.11,
      h,
      h + 0.06,
      s.blink(tint, 2 + i),
    );
  });
  // The arm: shoulder, upper arm along d, elbow, diagonal forearm, wrist.
  const [armH0, armH1] = [1.95, 2.1];
  const elbowD = 0;
  const drum = (a: number, d: number) =>
    k.cylinder(a, d, armH0 - 0.03, top - 0.02, 0.11, 12, s.dark);
  drum(colA, colD);
  k.bevelBox(
    colA - 0.08,
    colA + 0.08,
    colD,
    elbowD,
    armH0,
    armH1,
    HOUSING_BEVEL,
    s.body,
  );
  drum(colA, elbowD);
  const reach = Math.hypot(chairA - colA, chairD - elbowD);
  const fore = yawed(
    f,
    colA,
    elbowD,
    Math.atan2(colA - chairA, chairD - elbowD),
  );
  kitAt(fore).bevelBox(
    -0.07,
    0.07,
    0,
    reach,
    armH0,
    armH1,
    HOUSING_BEVEL,
    s.body,
  );
  drum(chairA, chairD);
  // The emitter, pointing down at the seat.
  const [barrelH0, barrelR] = [1.35, 0.12];
  k.cylinder(chairA, chairD, barrelH0, armH0, barrelR, 12, s.metal);
  for (let i = 0; i < 4; i++)
    k.ring(
      chairA,
      chairD,
      1.48 + i * 0.12,
      barrelR + 0.025,
      0.025,
      4,
      12,
      s.dark,
      "up",
    );
  lens(k, s, chairA, chairD, barrelH0, 0.09, LENS_RED, 0, "down");
  k.cylinder(
    chairA + barrelR + 0.02,
    chairD,
    1.3,
    1.9,
    0.01,
    6,
    s.blink(LENS_RED, 1),
  );
};

/** The optics kinds' recipes. */
export const OPTICS_RECIPES = {
  turret,
  "black-slab": blackSlab,
  "eye-panel": eyePanel,
  "photo-console": photoConsole,
  "laser-desk": laserDesk,
} satisfies Record<
  Extract<
    HeroKind,
    "turret" | "black-slab" | "eye-panel" | "photo-console" | "laser-desk"
  >,
  HeroRecipe
>;
