/**
 * The optics heroes' recipes: the turret, the black slab, the eye panel,
 * the photo console and the laser desk. What they share is a lens or a
 * screen: a glowing eye or lens set in a housing (the turret, the eye
 * panel, the laser desk's emitter), a screen bank on a desk (the photo
 * console, the laser desk's terminal), and the slab, which is all housing
 * and no light.
 *
 * This file sets the hero style the other batches follow:
 * - The footprint and top come from `heroHalf`; the heights and positions
 *   a kind is built to are the plan's, named in an `as const` table right
 *   above its recipe (`TURRET`, `EYE_PANEL`, `PHOTO_CONSOLE`,
 *   `LASER_DESK`), so a recipe reads its numbers by name.
 * - Housings are bevelled boxes (`HOUSING_BEVEL`, a thin chamfer that
 *   catches the light), except where a crisp edge is the point (the slab).
 * - Round parts use few facets: 10 to 16 around a body, a housing or a
 *   drum, 6 to 8 around a small post, pad or caster, 4 around a ring's
 *   tube, so every hero stays under 1500 triangles.
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
  discOutline,
  profileAlong,
  tiltedBar,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import { heroHalf, type HeroRecipe } from "./common";

/** The turret's shell: a clean glossy white, the same in every look. */
const WHITE_SHELL: Rgb = [0.93, 0.93, 0.9];

/**
 * A deep warning red, for every red light here: the lenses, the laser's
 * beam guide and the red status lights.
 */
const SIGNAL_RED: Rgb = [1.0, 0.1, 0.06];

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

/** Every recipe's main frame: at the origin, facing north (`frameAt([0, 0, 0], 0)`). */
const ORIGIN = frameAt([0, 0, 0], 0);

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
    k.extrude(
      discOutline(a, h, radius, LENS_SIDES),
      d - tube,
      d + tube / 2,
      light,
    );
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
 * The turret's measures, in metres: the legs' hub and foot ends (`[d, h]`
 * in a leg's own side view, the foot's `d` measured in from the box's
 * edge), their width, the foot pads, the hub, the egg's profile
 * (`[r, h]`, widest at 0.85), the seam ring's height and radius, and the
 * eye: its height, its plane (`d`, just proud of the shell at the lens's
 * foot), its socket's back and radius, and the lens radius.
 */
const TURRET = {
  legHub: [0.03, 0.56],
  legFootIn: 0.06,
  legFootH: 0.03,
  legWidth: 0.035,
  pad: { r: 0.045, h: 0.03 },
  hub: { r: 0.07, h0: 0.48, h1: 0.6 },
  egg: [
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
  eggSides: 10,
  seam: { h: 0.92, r: 0.29, tube: 0.012 },
  eye: { h: 1.02, d: 0.285, socketBack: 0.15, socketR: 0.09, r: 0.06 },
} as const;

/**
 * The turret: a white egg on three thin splayed legs, with one red eye.
 * The legs run from a dark hub under the egg (h 0.55) out to near the
 * box's edge on the floor, 120 degrees apart with one pointing back
 * (`-d`), each a `tiltedBar` in a yawed frame's side view on a small foot
 * pad. The egg is a 10-sided lathe from h 0.5 to 1.3, widest (r 0.3) at
 * h 0.85, with a dark seam ring at h 0.92. The eye sits in a dark socket
 * on the front at h 1.02, its plane just proud of the shell at the lens's
 * foot (the shell leans back above it, so the socket shows there): a red
 * `lens` of r 0.06, blink group 0, which the breathe bank pulses slowly.
 */
const turret: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const T = TURRET;
  const { d1 } = heroHalf(kind, variant);
  const [d0, h0] = T.legHub;
  const [dF, hF] = [d1 - T.legFootIn, T.legFootH];
  const leg = tiltedBar(
    (d0 + dF) / 2,
    (h0 + hF) / 2,
    Math.atan2(hF - h0, dF - d0),
    Math.hypot(dF - d0, hF - h0),
    T.legWidth,
  );
  const half = T.legWidth / 2;
  for (const yaw of [Math.PI, Math.PI / 3, -Math.PI / 3]) {
    const fl = yawed(ORIGIN, 0, 0, yaw);
    profileAlong(kitAt, fl, leg, -half, half, s.dark);
    kitAt(fl).cylinder(0, dF, 0, T.pad.h, T.pad.r, 8, s.dark);
  }
  k.cylinder(0, 0, T.hub.h0, T.hub.h1, T.hub.r, 10, s.dark);
  k.lathe(0, 0, T.egg, T.eggSides, s.tinted(WHITE_SHELL));
  k.ring(0, 0, T.seam.h, T.seam.r, T.seam.tube, 4, 20, s.dark, "up");
  const e = T.eye;
  k.extrude(
    discOutline(0, e.h, e.socketR, LENS_SIDES),
    e.socketBack,
    e.d,
    s.dark,
  );
  lens(k, s, 0, e.d, e.h, e.r, SIGNAL_RED, 0);
};

/**
 * The black slab: one plain box over the whole footprint from the floor
 * to its top, 1 : 4 : 9 (0.3 by 1.2 by 2.7 m), near black and lit. No
 * bevel and no light: its crisp edges are the whole of it. Every measure
 * is the catalogue's, so it needs no table.
 */
const blackSlab: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw, d0, d1, top } = heroHalf(kind, variant);
  k.box(-hw, hw, d0, d1, 0, top, s.tinted(SLAB_BLACK));
};

/**
 * The eye panel's measures, in metres: the panel's inset from the
 * footprint's sides, its depth, bottom and the dark face's depth; the
 * lens housing's height, barrel radius, rim ring (`d` of its middle,
 * radius and tube, its front at 0.2) and the lens radius; the grille's
 * six bars (lowest bottom, pitch, height, half width) and the name plate.
 */
const EYE_PANEL = {
  inset: 0.05,
  depth: 0.08,
  bottom: 0.4,
  face: 0.09,
  eye: {
    h: 1.35,
    barrelR: 0.15,
    rimD: 0.17,
    rimR: 0.13,
    rimTube: 0.03,
    r: 0.09,
  },
  grille: { h0: 0.8, pitch: 0.055, h: 0.025, half: 0.2 },
  plate: { half: 0.22, h0: 1.7, h1: 1.8 },
} as const;

/**
 * The eye panel, flush on its wall: a tall brushed-metal panel (d 0 to
 * 0.08, h 0.4 to the top, the width less 0.05 each side) with a dark inset
 * face, a round metal lens housing standing out to d 0.2 at h 1.35 with a
 * red `lens` (r 0.09, blink group 0) in it, a speaker grille of six thin
 * bars from h 0.8 to 1.1, a blank name plate at h 1.75 and four bolts.
 */
const eyePanel: HeroRecipe = ({ k, s, variant, kind }) => {
  const P = EYE_PANEL;
  const { hw, top } = heroHalf(kind, variant);
  const pw = hw - P.inset;
  const face = P.face;
  k.bevelBox(-pw, pw, 0, P.depth, P.bottom, top, HOUSING_BEVEL, s.metal);
  k.box(
    -pw + 0.06,
    pw - 0.06,
    P.depth,
    face,
    P.bottom + 0.1,
    top - 0.1,
    s.dark,
  );
  for (const a of [-pw + 0.03, pw - 0.03])
    for (const h of [P.bottom + 0.05, top - 0.05])
      k.box(a - 0.015, a + 0.015, P.depth, face, h - 0.015, h + 0.015, s.dark);
  // The housing: a short metal barrel with a rim ring whose front is at 0.2.
  const e = P.eye;
  k.extrude(discOutline(0, e.h, e.barrelR, 16), face, e.rimD, s.metal);
  k.ring(0, e.rimD, e.h, e.rimR, e.rimTube, 4, 16, s.metal, "inward");
  lens(k, s, 0, e.rimD, e.h, e.r, SIGNAL_RED, 0);
  const g = P.grille;
  for (let i = 0; i < 6; i++) {
    const h = g.h0 + i * g.pitch;
    k.box(-g.half, g.half, face, face + 0.015, h, h + g.h, s.metal);
  }
  const n = P.plate;
  k.box(-n.half, n.half, face, face + 0.01, n.h0, n.h1, s.panel);
};

/**
 * The photo console's measures, in metres: the tower's half width and
 * depth; the deck's high back edge (`d`, `h`) and its low front edge
 * (`d`, `h`); the toe recess; the screen, its bezel and its visor; the
 * buttons beside the screen; the keypad grid; the print tray; the
 * trackball; and the status lights' row.
 */
const PHOTO_CONSOLE = {
  towerHalf: 0.6,
  tower: 0.4,
  deckBack: [0.45, 0.95],
  deckFront: [0.9, 0.75],
  toe: { depth: 0.06, h: 0.1 },
  screen: { half: 0.35, h0: 1.0, h1: 1.7 },
  bezel: { half: 0.42, depth: 0.04, h0: 0.96, h1: 1.74 },
  visor: { half: 0.45, depth: 0.1, h0: 1.76, h1: 1.84 },
  buttons: { a: 0.515, hs: [1.2, 1.35, 1.5] },
  keys: { a0: -0.7, d0: 0.55, pitchA: 0.1, pitchD: 0.09, a: 0.07, d: 0.065 },
  tray: { a0: -0.2, a1: 0.25, d0: 0.52, d1: 0.82 },
  ball: { a: 0.55, d: 0.66, pad: 0.07, r: 0.05 },
  lights: { a0: -0.5, pitch: 0.2, h0: 0.66, h1: 0.7 },
} as const;

/** The photo console's sloped deck: its top at depth `d`, from its back edge down to its front edge. */
function deckTop(d: number): number {
  const [db, hb] = PHOTO_CONSOLE.deckBack;
  const [df, hf] = PHOTO_CONSOLE.deckFront;
  return hb + ((d - db) * (hf - hb)) / (df - db);
}

/** How far a small status light stands out of its face, in metres. */
const LIGHT_DEPTH = 0.01;

/** The six status lights along the photo console's front edge, one per blink group. */
const CONSOLE_LIGHTS: readonly Rgb[] = [
  STATUS_GREEN,
  STATUS_GREEN,
  STATUS_AMBER,
  STATUS_GREEN,
  SIGNAL_RED,
  STATUS_AMBER,
];

/**
 * The photo console, backed against its wall: a tall tower (d 0 to 0.4,
 * up to the top) holding a large square screen facing `+d` (h 1.0 to 1.7,
 * a dark bezel, a sepia-grey `s.glow` screen, a visor above and two rows
 * of buttons beside it), and in front of it a console desk whose deck
 * slopes from 0.95 at d 0.45 down to 0.75 at the front edge (d 0.9),
 * carrying a keypad (a grid of small keys following the slope), a dark
 * print tray and a trackball on a dark pad, both following the slope too.
 * Six status lights along the deck's front edge blink in groups 0 to 5 of
 * the status bank.
 */
const photoConsole: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const C = PHOTO_CONSOLE;
  const { hw, d1, top } = heroHalf(kind, variant);
  // The desk's front face stands a light's depth behind the footprint's
  // edge, so the status lights on it end exactly on that edge.
  const front = d1 - LIGHT_DEPTH;
  const tower = C.tower;
  k.bevelBox(
    -C.towerHalf,
    C.towerHalf,
    0,
    tower,
    0,
    top,
    HOUSING_BEVEL * 1.5,
    s.body,
  );
  // The desk, with a toe recess under its front edge.
  const dw = hw - 0.02;
  const [backD] = C.deckBack;
  profileAlong(
    kitAt,
    ORIGIN,
    [
      [tower, 0],
      [front - C.toe.depth, 0],
      [front - C.toe.depth, C.toe.h],
      [front, C.toe.h],
      [front, deckTop(front)],
      [backD, deckTop(backD)],
      [tower, deckTop(backD)],
    ],
    -dw,
    dw,
    s.body,
  );
  // The screen.
  const b = C.bezel;
  k.box(-b.half, b.half, tower, tower + b.depth, b.h0, b.h1, s.dark);
  k.panel(
    -C.screen.half,
    C.screen.half,
    tower + b.depth + DECAL_LIFT,
    C.screen.h0,
    C.screen.h1,
    s.glow(SEPIA_GREY),
  );
  const v = C.visor;
  k.bevelBox(-v.half, v.half, tower, tower + v.depth, v.h0, v.h1, 0.01, s.body);
  for (const a of [-C.buttons.a, C.buttons.a])
    for (const h of C.buttons.hs)
      k.box(a - 0.03, a + 0.03, tower, tower + 0.015, h, h + 0.04, s.dark);
  // On the deck: keys, tray and the ball's pad follow the slope, as thin
  // sloped slabs from `below` under the deck to `above` over it.
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
      ORIGIN,
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
  const K = C.keys;
  for (let r = 0; r < 3; r++)
    for (let c = 0; c < 4; c++) {
      const e0 = K.d0 + r * K.pitchD;
      const a0 = K.a0 + c * K.pitchA;
      slab(a0, a0 + K.a, e0, e0 + K.d, 0.005, 0.015, s.dark);
    }
  slab(C.tray.a0, C.tray.a1, C.tray.d0, C.tray.d1, 0.01, 0.01, s.dark);
  // The trackball: a dark pad flush on the deck, and the ball, whose foot
  // reaches below the deck at its front so it never floats on the slope.
  const t = C.ball;
  slab(
    t.a - t.pad,
    t.a + t.pad,
    t.d - t.pad,
    t.d + t.pad,
    0.005,
    0.008,
    s.dark,
  );
  const th = deckTop(t.d);
  const foot = th - (deckTop(t.d) - deckTop(t.d + t.r)) - 0.005;
  k.lathe(
    t.a,
    t.d,
    [
      [0, foot],
      [t.r * 0.9, foot],
      [t.r, th + 0.02],
      [t.r * 0.9, th + 0.04],
      [t.r * 0.56, th + 0.055],
      [0, th + 0.06],
    ],
    10,
    s.panel,
  );
  const L = C.lights;
  CONSOLE_LIGHTS.forEach((tint, i) => {
    const a = L.a0 + i * L.pitch;
    k.box(a - 0.025, a + 0.025, front, d1, L.h0, L.h1, s.blink(tint, i));
  });
};

/**
 * The laser desk's measures, in metres: the desk (its top's depth range
 * and height, the slab's underside), the monitor and its screen, the
 * keyboard, the chair's place, the column (its place, the foot plate's
 * half sizes and height, its own half side and top), the column lights,
 * the arm (its height band, the elbow's `d`, the drums' radius), the
 * barrel (its foot and radius, the fins' lowest height and pitch), the
 * lens radius and the beam guide rod.
 */
const LASER_DESK = {
  desk: { d0: -0.3, d1: 0.5, under: 0.7, top: 0.74 },
  monitor: { a0: 0.1, a1: 0.7, d0: -0.25, d1: 0.15, top: 1.2 },
  screen: { a0: 0.16, a1: 0.64, h0: 0.83, h1: 1.13 },
  keyboard: { a0: 0.15, a1: 0.65, d0: 0.2, d1: 0.4 },
  chair: { a: 0.3, d: 1.0 },
  column: {
    a: -0.9,
    d: -1.3,
    plateA: 0.25,
    plateD: 0.18,
    plateH: 0.06,
    half: 0.1,
    top: 2.1,
  },
  lights: { h0: 1.2, pitch: 0.12, h: 0.06 },
  arm: { h0: 1.95, h1: 2.1, elbowD: 0, drumR: 0.11 },
  barrel: { h0: 1.35, r: 0.12, fin0: 1.48, finPitch: 0.12 },
  lensR: 0.09,
  rod: { r: 0.01, h0: 1.3, h1: 1.9 },
} as const;

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
 *   looking down at the seat, and a thin beam guide rod (r 0.01) touching
 *   its side from h 1.9 to 1.3.
 * Lights: the lens is blink group 0, the beam guide group 1 and the column
 * lights groups 2 to 4, all in the breathe bank.
 */
const laserDesk: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const L = LASER_DESK;
  const { hw, top } = heroHalf(kind, variant);
  // The desk.
  const D = L.desk;
  k.bevelBox(-hw, hw, D.d0, D.d1, D.under, D.top, 0.01, s.body);
  for (const a of [-hw + 0.05, hw - 0.05])
    for (const d of [D.d0 + 0.05, D.d1 - 0.05])
      k.box(a - 0.025, a + 0.025, d - 0.025, d + 0.025, 0, D.under, s.metal);
  const M = L.monitor;
  k.bevelBox(M.a0, M.a1, M.d0, M.d1, D.top, M.top, HOUSING_BEVEL, s.body);
  k.box(
    M.a0 + 0.03,
    M.a1 - 0.03,
    M.d1,
    M.d1 + 0.01,
    D.top + 0.06,
    M.top - 0.04,
    s.dark,
  );
  const sc = L.screen;
  k.panel(
    sc.a0,
    sc.a1,
    M.d1 + 0.01 + DECAL_LIFT,
    sc.h0,
    sc.h1,
    s.glow(DARK_CYAN),
  );
  const kb = L.keyboard;
  k.box(kb.a0, kb.a1, kb.d0, kb.d1, D.top, D.top + 0.025, s.dark);
  // The chair, pulled up to the desk.
  const { a: chairA, d: chairD } = L.chair;
  officeChair(kitAt, ORIGIN, s, chairA, chairD, 2);
  // The column.
  const C = L.column;
  k.bevelBox(
    C.a - C.plateA,
    C.a + C.plateA,
    C.d - C.plateD,
    C.d + C.plateD,
    0,
    C.plateH,
    HOUSING_BEVEL,
    s.dark,
  );
  k.bevelBox(
    C.a - C.half,
    C.a + C.half,
    C.d - C.half,
    C.d + C.half,
    C.plateH,
    C.top,
    HOUSING_BEVEL,
    s.body,
  );
  [STATUS_GREEN, STATUS_AMBER, SIGNAL_RED].forEach((tint, i) => {
    const h = L.lights.h0 + i * L.lights.pitch;
    const face = C.d + C.half;
    k.box(
      C.a - 0.04,
      C.a + 0.04,
      face,
      face + LIGHT_DEPTH,
      h,
      h + L.lights.h,
      s.blink(tint, 2 + i),
    );
  });
  // The arm: shoulder, upper arm along d, elbow, diagonal forearm, wrist.
  const A = L.arm;
  const drum = (a: number, d: number) =>
    k.cylinder(a, d, A.h0 - 0.03, top - 0.02, A.drumR, 12, s.dark);
  drum(C.a, C.d);
  k.bevelBox(
    C.a - 0.08,
    C.a + 0.08,
    C.d,
    A.elbowD,
    A.h0,
    A.h1,
    HOUSING_BEVEL,
    s.body,
  );
  drum(C.a, A.elbowD);
  const reach = Math.hypot(chairA - C.a, chairD - A.elbowD);
  const fore = yawed(
    ORIGIN,
    C.a,
    A.elbowD,
    Math.atan2(C.a - chairA, chairD - A.elbowD),
  );
  kitAt(fore).bevelBox(
    -0.07,
    0.07,
    0,
    reach,
    A.h0,
    A.h1,
    HOUSING_BEVEL,
    s.body,
  );
  drum(chairA, chairD);
  // The emitter, pointing down at the seat.
  const B = L.barrel;
  k.cylinder(chairA, chairD, B.h0, A.h0, B.r, 12, s.metal);
  for (let i = 0; i < 4; i++)
    k.ring(
      chairA,
      chairD,
      B.fin0 + i * B.finPitch,
      B.r + 0.025,
      0.025,
      4,
      12,
      s.dark,
      "up",
    );
  lens(k, s, chairA, chairD, B.h0, L.lensR, SIGNAL_RED, 0, "down");
  // The rod touches the barrel: the barrel's 12 facets put a corner on +a.
  k.cylinder(
    chairA + B.r + L.rod.r,
    chairD,
    L.rod.h0,
    L.rod.h1,
    L.rod.r,
    6,
    s.blink(SIGNAL_RED, 1),
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
