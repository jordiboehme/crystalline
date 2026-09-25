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
import { LAYER } from "../../layers";
import type { Rgb } from "../../looks";
import {
  discOutline,
  profileAlong,
  tiltedBar,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import {
  heroHalf,
  STATUS_AMBER,
  STATUS_GREEN,
  type HeroRecipe,
} from "./common";

/** The turret's shell: a clean glossy white, the same in every look. */
const WHITE_SHELL: Rgb = [0.92, 0.92, 0.9];

/**
 * The turret's black trim: its legs, hub, seam, eye socket and the gaps
 * round its side panels, a near black with a hint of blue.
 */
const TURRET_BLACK: Rgb = [0.12, 0.12, 0.13];

/**
 * A deep warning red, for every red light here: the lenses, the laser's
 * beam guide and the red status lights.
 */
const SIGNAL_RED: Rgb = [1.0, 0.1, 0.06];

/** The slab's finish: a flat matte black (lit, so it still takes a faint edge). */
const SLAB_BLACK: Rgb = [0.02, 0.02, 0.02];

/**
 * The eye panel's face: a warm brushed metal, champagne rather than
 * steel, drawn on the metal layer so its grain shows.
 */
const FACEPLATE_CHAMPAGNE: Rgb = [0.75, 0.7, 0.6];

/** The eye panel's round bezel, name badge and grille backing: a deep black. */
const BEZEL_BLACK: Rgb = [0.05, 0.05, 0.055];

/** The eye panel's grille slats: a plain brushed aluminium. */
const GRILLE_GREY: Rgb = [0.55, 0.55, 0.57];

/** The small bright dot in the middle of the eye panel's lens. */
const LENS_YELLOW: Rgb = [1.0, 0.85, 0.25];

/** The blue field on the right of the eye panel's blank name badge. */
const BADGE_BLUE: Rgb = [0.16, 0.32, 0.72];

/** The photo console's casing: an old cream plastic, gone warm with age. */
const CASING_CREAM: Rgb = [0.84, 0.79, 0.66];

/** The photo console's base, cheeks and knobs: a dark brown, like old wood-effect trim. */
const CASING_BROWN: Rgb = [0.24, 0.16, 0.1];

/** The photo console's picture: a cold grey blue, a still under a scanning tube. */
const SCREEN_GREY_BLUE: Rgb = [0.44, 0.54, 0.62];

/** The photo console's grid lines over the picture: a paler blue. */
const SCREEN_GRID: Rgb = [0.72, 0.82, 0.9];

/** The laser's barrel and nose: a matte black. */
const LASER_BLACK: Rgb = [0.07, 0.07, 0.08];

/** The laser rig's column, arm and foot: an industrial off-white grey. */
const RIG_GREY: Rgb = [0.65, 0.65, 0.63];

/** The laser desk's monitor: a dark cyan glow, a terminal late at night. */
const DARK_CYAN: Rgb = [0.1, 0.42, 0.48];

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
 * edge), their width, the foot pads and the hub; the egg's profile
 * (`[r, h]`, a flat-ish base, widest at the shoulders near h 1.0) and its
 * facets; the vertical seam (how far it stands proud of the shell, the
 * share of the radius its inner edge keeps, its half thickness and its
 * foot); the side panels' seams (their foot and head, and the yaw of the
 * two planes they lie in, a lathe corner's multiple so they ride ridges);
 * and the eye: its height, its plane (`d`, just proud of the
 * shell), its socket's back and radius, and the lens radius.
 */
const TURRET = {
  legHub: [0.03, 0.42],
  legFootIn: 0.06,
  legFootH: 0.03,
  legWidth: 0.03,
  pad: { r: 0.04, h: 0.03 },
  hub: { r: 0.06, h0: 0.36, h1: 0.46 },
  egg: [
    [0, 0.38],
    [0.1, 0.39],
    [0.17, 0.44],
    [0.215, 0.53],
    [0.25, 0.66],
    [0.27, 0.82],
    [0.276, 0.98],
    [0.26, 1.1],
    [0.215, 1.2],
    [0.14, 1.27],
    [0, 1.3],
  ],
  eggSides: 12,
  seam: { out: 0.008, inner: 0.9, half: 0.005, h0: 0.4, apex: 1.285 },
  pod: { h0: 0.52, h1: 1.16, yaw: Math.PI / 3 },
  eye: { h: 1.04, d: 0.27, socketBack: 0.15, socketR: 0.09, r: 0.06 },
} as const;

/** The turret egg's radius at height `h`, read off its profile by straight lines. */
function eggRadius(h: number): number {
  const p = TURRET.egg;
  for (let i = 0; i + 1 < p.length; i++) {
    const [r0, h0] = p[i] ?? [0, 0];
    const [r1, h1] = p[i + 1] ?? [0, 0];
    if (h >= h0 && h <= h1) return r0 + ((h - h0) * (r1 - r0)) / (h1 - h0);
  }
  return 0;
}

/**
 * The turret's vertical seam as two outlines in the side view (`[d, h]`),
 * to push a hair across the plane `a = 0`: `below`, on the front from the
 * seam's foot up to the eye socket, and `over`, from the socket's top up
 * over the crown and down the back to the foot. Each is a band whose outer
 * edge stands `seam.out` proud of the egg (pushed out sideways only, so the
 * crown stays at the top) and whose inner edge lies inside the shell. The
 * lathe puts a corner on `+d` and `-d`, so the band rides that ridge.
 */
function seamOutlines(): {
  below: [number, number][];
  over: [number, number][];
} {
  const { seam, eye, egg } = TURRET;
  const crown = egg[egg.length - 1]?.[1] ?? 0;
  const neck = egg[egg.length - 2]?.[1] ?? 0;
  const heights = (h0: number, h1: number) => [
    h0,
    ...egg.map(([, h]) => h).filter((h) => h > h0 && h < h1),
    h1,
  ];
  const out = (h: number) => eggRadius(h) + seam.out;
  const inn = (h: number) => eggRadius(h) * seam.inner;
  const lo = eye.h - eye.socketR;
  const hi = eye.h + eye.socketR;
  const below = seamBand(seam.h0, lo, 1);
  const over: [number, number][] = [
    ...heights(hi, crown).map((h): [number, number] => [out(h), h]),
    ...heights(seam.h0, crown)
      .reverse()
      .map((h): [number, number] => [-out(h), h]),
    ...heights(seam.h0, neck).map((h): [number, number] => [-inn(h), h]),
    [0, seam.apex],
    ...heights(hi, neck)
      .reverse()
      .map((h): [number, number] => [inn(h), h]),
  ];
  return { below, over };
}

/**
 * A straight run of the turret's seam in the side view (`[d, h]`), from
 * `h0` up to `h1` on the `+d` face (`side` 1) or the `-d` face (`side`
 * -1): a band whose outer edge stands `seam.out` proud of the egg and
 * whose inner edge lies inside the shell, pushed across a plane through
 * the egg's axis.
 */
function seamBand(h0: number, h1: number, side: 1 | -1): [number, number][] {
  const { seam, egg } = TURRET;
  const hs = [h0, ...egg.map(([, h]) => h).filter((h) => h > h0 && h < h1), h1];
  return [
    ...hs.map((h): [number, number] => [side * (eggRadius(h) + seam.out), h]),
    ...[...hs]
      .reverse()
      .map((h): [number, number] => [side * eggRadius(h) * seam.inner, h]),
  ];
}

/**
 * The turret: a glossy white egg on three thin black legs, with one red
 * eye.
 * - The legs run from a black hub under the egg (h 0.42) out to near the
 *   box's edge on the floor, 120 degrees apart with one pointing back
 *   (`-d`), each a `tiltedBar` in a yawed frame's side view on a small
 *   foot pad.
 * - The egg is a 12-sided lathe from h 0.38 to 1.3 with a flat-ish base,
 *   widest (r 0.276) at the shoulders near h 1.0.
 * - A thin black seam splits it front to back in the plane `a = 0`, over
 *   the crown and down both faces, broken only by the eye socket.
 * - Two more seams on each side, 60 and 120 degrees round from the front
 *   and running from h 0.52 to 1.16, mark off a side panel on `+a` and on
 *   `-a`, so the sides look as if they could swing open.
 * - The eye sits high on the front (h 1.04) in a black socket: a red
 *   `lens` of r 0.06, blink group 0, which the breathe bank pulses slowly.
 */
const turret: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const T = TURRET;
  const black = s.tinted(TURRET_BLACK);
  const white = s.tinted(WHITE_SHELL);
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
    profileAlong(kitAt, fl, leg, -half, half, black);
    kitAt(fl).cylinder(0, dF, 0, T.pad.h, T.pad.r, 8, black);
  }
  k.cylinder(0, 0, T.hub.h0, T.hub.h1, T.hub.r, 10, black);
  k.lathe(0, 0, T.egg, T.eggSides, white);
  const { below, over } = seamOutlines();
  for (const band of [below, over])
    profileAlong(kitAt, ORIGIN, band, -T.seam.half, T.seam.half, black);
  const P = T.pod;
  for (const yaw of [P.yaw, -P.yaw]) {
    const fp = yawed(ORIGIN, 0, 0, yaw);
    for (const side of [1, -1] as const)
      profileAlong(
        kitAt,
        fp,
        seamBand(P.h0, P.h1, side),
        -T.seam.half,
        T.seam.half,
        black,
      );
  }
  const e = T.eye;
  k.extrude(
    discOutline(0, e.h, e.socketR, LENS_SIDES),
    e.socketBack,
    e.d,
    black,
  );
  lens(k, s, 0, e.d, e.h, e.r, SIGNAL_RED, 0);
};

/**
 * The black slab: one plain box over the whole footprint from the floor
 * to its top, exactly 1 : 4 : 9 (0.3 by 1.2 by 2.7 m), a flat matte black
 * and lit. No bevel, no seam, no light: its crisp edges are the whole of
 * it. Every measure is the catalogue's, so it needs no table.
 */
const blackSlab: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw, d0, d1, top } = heroHalf(kind, variant);
  k.box(-hw, hw, d0, d1, 0, top, s.tinted(SLAB_BLACK));
};

/**
 * The eye panel's measures, in metres: the faceplate's half width, depth
 * and bottom (a portrait plate of about 3 : 1, 0.6 by 1.8 m); the blank
 * name badge at the top (half width, heights, depth, its blue field's `a`
 * range, lift and inset); the eye (its height, the black bezel disc's
 * radius and depth, the lens radius, the red dome's radius and how far it
 * stands out of the bezel, the yellow centre dot's radius and reach); and
 * the grille (half width, heights, backing depth, the slats' count, first
 * height, pitch, height and depth).
 */
const EYE_PANEL = {
  half: 0.3,
  depth: 0.05,
  bottom: 0.4,
  badge: {
    half: 0.17,
    h0: 1.98,
    h1: 2.07,
    depth: 0.012,
    blue: [0.06, 0.155],
    lift: 0.004,
    inset: 0.014,
  },
  eye: {
    h: 1.62,
    bezelR: 0.26,
    bezel: 0.03,
    r: 0.14,
    domeR: 0.085,
    domeOut: 0.075,
    dotR: 0.02,
    dotOut: 0.085,
  },
  grille: {
    half: 0.16,
    h0: 0.78,
    h1: 1.2,
    back: 0.008,
    slats: 7,
    first: 0.8,
    pitch: 0.06,
    slat: 0.028,
    slatDepth: 0.008,
  },
} as const;

/**
 * The eye panel, flush on its wall: a tall portrait faceplate of warm
 * brushed metal (d 0 to 0.05, h 0.4 to the top, 0.6 m wide), and on it,
 * from the top down:
 * - a blank black name badge with a blue field on its right, no text;
 * - the eye, high on the plate (h 1.62): a black round bezel disc, a red
 *   `lens` (r 0.14) in it, a smaller red dome standing out of the lens
 *   like a fisheye's glass, and a small yellow dot at its very middle, all
 *   in blink group 0 so the whole eye breathes as one;
 * - a speaker grille of seven aluminium slats on a black backing.
 */
const eyePanel: HeroRecipe = ({ k, s, variant, kind }) => {
  const P = EYE_PANEL;
  const { top } = heroHalf(kind, variant);
  const face = P.depth;
  const black = s.tinted(BEZEL_BLACK);
  k.bevelBox(
    -P.half,
    P.half,
    0,
    face,
    P.bottom,
    top,
    HOUSING_BEVEL,
    s.tinted(FACEPLATE_CHAMPAGNE, LAYER.metal),
  );
  const b = P.badge;
  const [blue0, blue1] = b.blue;
  k.box(-b.half, b.half, face, face + b.depth, b.h0, b.h1, black);
  k.box(
    blue0,
    blue1,
    face + b.depth,
    face + b.depth + b.lift,
    b.h0 + b.inset,
    b.h1 - b.inset,
    s.tinted(BADGE_BLUE),
  );
  // The eye: the bezel disc, the lens seated on it, the dome and the dot.
  const e = P.eye;
  const front = face + e.bezel;
  k.extrude(discOutline(0, e.h, e.bezelR, 16), face, front, black);
  const tube = Math.max(BEZEL_MIN, e.r * BEZEL_SHARE);
  lens(k, s, 0, front + tube, e.h, e.r, SIGNAL_RED, 0);
  k.extrude(
    discOutline(0, e.h, e.domeR, LENS_SIDES),
    front,
    front + e.domeOut,
    s.blink(SIGNAL_RED, 0),
  );
  k.extrude(
    discOutline(0, e.h, e.dotR, 8),
    front,
    front + e.dotOut,
    s.blink(LENS_YELLOW, 0),
  );
  const g = P.grille;
  const slat = s.tinted(GRILLE_GREY, LAYER.metal);
  k.box(-g.half, g.half, face, face + g.back, g.h0, g.h1, black);
  for (let i = 0; i < g.slats; i++) {
    const h = g.first + i * g.pitch;
    k.box(
      -g.half + 0.015,
      g.half - 0.015,
      face + g.back,
      face + g.back + g.slatDepth,
      h,
      h + g.slat,
      slat,
    );
  }
};

/**
 * The photo console's measures, in metres: the brown base's top; the
 * cream deck's high back edge (`d`, `h`) and its low front edge (`d`,
 * `h`); the toe recess; the hood (half width, the brown cheeks' width, the
 * lip under its screen face, the face's top corner `[d, h]` and the back
 * of its roof); the screen's bezel and picture on the sloped face (half
 * widths, heights and how far each stands off the face) and the grid lines
 * over it; the print slot in the lip; the chunky buttons; the knobs; and
 * the trackball.
 */
const PHOTO_CONSOLE = {
  baseTop: 0.7,
  deckBack: [0.45, 0.95],
  deckFront: [0.9, 0.75],
  toe: { depth: 0.06, h: 0.1 },
  hood: {
    half: 0.52,
    cheek: 0.06,
    lip: 1.0,
    faceTop: [0.33, 1.76],
    roof: 0.25,
  },
  bezel: { half: 0.43, h0: 1.05, h1: 1.72, off0: -0.003, off1: 0.012 },
  screen: { half: 0.36, h0: 1.1, h1: 1.67, off1: 0.016 },
  grid: { width: 0.008, off1: 0.019 },
  slot: { half: 0.2, h0: 0.962, h1: 0.99, depth: 0.006 },
  buttons: { a0: -0.76, pitch: 0.13, a: 0.1, d0: 0.58, d: 0.09, above: 0.035 },
  knobs: { as: [0.1, 0.24], d: 0.66, r: 0.035, h: 0.035 },
  ball: { a: 0.56, d: 0.66, pad: 0.1, r: 0.065 },
} as const;

/** The photo console's sloped deck: its top at depth `d`, from its back edge down to its front edge. */
function deckTop(d: number): number {
  const [db, hb] = PHOTO_CONSOLE.deckBack;
  const [df, hf] = PHOTO_CONSOLE.deckFront;
  return hb + ((d - db) * (hf - hb)) / (df - db);
}

/**
 * A point of the photo console's sloped screen face in the side view
 * (`[d, h]`): on the face's line at height `h`, moved `off` out along the
 * face's normal (negative is into the hood).
 */
function screenFace(h: number, off: number): [number, number] {
  const [d0] = PHOTO_CONSOLE.deckBack;
  const { lip, faceTop } = PHOTO_CONSOLE.hood;
  const [d1, h1] = faceTop;
  const run = d1 - d0;
  const rise = h1 - lip;
  const len = Math.hypot(run, rise);
  const d = d0 + ((h - lip) * run) / rise;
  return [d + (off * rise) / len, h - (off * run) / len];
}

/** The six chunky lit buttons on the photo console's deck, one per blink group. */
const CONSOLE_LIGHTS: readonly Rgb[] = [
  STATUS_GREEN,
  STATUS_GREEN,
  STATUS_AMBER,
  STATUS_GREEN,
  SIGNAL_RED,
  STATUS_AMBER,
];

/**
 * The photo console, backed against its wall: a boxy old console in cream
 * and dark brown.
 * - A dark brown base cabinet (up to h 0.7, a toe recess under its front)
 *   carries a cream deck that slopes from 0.95 at d 0.45 down to 0.75 at
 *   the front edge (d 0.9).
 * - Behind the deck stands a big cream screen hood with brown cheeks, its
 *   face leaning back as it rises (from d 0.45 at h 1.0 to d 0.33 at h
 *   1.76), the roof sloping to the wall at the top.
 * - On that sloped face: a black bezel and a grey-blue picture (`s.glow`)
 *   with a pale grid of two lines each way over it; a dark print slot in
 *   the lip under it.
 * - On the deck: a row of six chunky lit buttons (blink groups 0 to 5 of
 *   the status bank), two brown knobs and a trackball on a brown pad, all
 *   following the slope.
 */
const photoConsole: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const C = PHOTO_CONSOLE;
  const { hw, d1, top } = heroHalf(kind, variant);
  const cream = s.tinted(CASING_CREAM);
  const brown = s.tinted(CASING_BROWN);
  const dw = hw - 0.02;
  const [backD, backH] = C.deckBack;
  // The base, with a toe recess under its front edge, and the deck on it.
  profileAlong(
    kitAt,
    ORIGIN,
    [
      [0, 0],
      [d1 - C.toe.depth, 0],
      [d1 - C.toe.depth, C.toe.h],
      [d1, C.toe.h],
      [d1, C.baseTop],
      [0, C.baseTop],
    ],
    -dw,
    dw,
    brown,
  );
  profileAlong(
    kitAt,
    ORIGIN,
    [
      [0, C.baseTop],
      [d1, C.baseTop],
      [d1, deckTop(d1)],
      [backD, backH],
      [0, backH],
    ],
    -dw,
    dw,
    cream,
  );
  // The hood, its cheeks and its sloped screen face.
  const H = C.hood;
  const hood: [number, number][] = [
    [0, backH],
    [backD, backH],
    [backD, H.lip],
    [H.faceTop[0], H.faceTop[1]],
    [H.roof, top],
    [0, top],
  ];
  profileAlong(kitAt, ORIGIN, hood, -H.half, H.half, cream);
  for (const side of [1, -1])
    profileAlong(
      kitAt,
      ORIGIN,
      hood,
      side * H.half,
      side * (H.half + H.cheek),
      brown,
    );
  const onFace = (
    a0: number,
    a1: number,
    h0: number,
    h1: number,
    off0: number,
    off1: number,
    sf: Surface,
  ) =>
    profileAlong(
      kitAt,
      ORIGIN,
      [
        screenFace(h0, off0),
        screenFace(h1, off0),
        screenFace(h1, off1),
        screenFace(h0, off1),
      ],
      a0,
      a1,
      sf,
    );
  const b = C.bezel;
  onFace(-b.half, b.half, b.h0, b.h1, b.off0, b.off1, s.tinted(BEZEL_BLACK));
  const sc = C.screen;
  onFace(
    -sc.half,
    sc.half,
    sc.h0,
    sc.h1,
    b.off1,
    sc.off1,
    s.glow(SCREEN_GREY_BLUE),
  );
  const grid = s.glow(SCREEN_GRID);
  const w = C.grid.width / 2;
  for (const third of [1, 2]) {
    const a = -sc.half + (third * 2 * sc.half) / 3;
    onFace(a - w, a + w, sc.h0, sc.h1, sc.off1, C.grid.off1, grid);
    const h = sc.h0 + (third * (sc.h1 - sc.h0)) / 3;
    onFace(-sc.half, sc.half, h - w, h + w, sc.off1, C.grid.off1, grid);
  }
  const sl = C.slot;
  k.box(-sl.half, sl.half, backD, backD + sl.depth, sl.h0, sl.h1, s.dark);
  // On the deck: buttons, knobs and the ball's pad follow the slope, as
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
  const B = C.buttons;
  CONSOLE_LIGHTS.forEach((tint, i) => {
    const a0 = B.a0 + i * B.pitch;
    slab(a0, a0 + B.a, B.d0, B.d0 + B.d, 0.005, B.above, s.blink(tint, i));
  });
  const n = C.knobs;
  for (const a of n.as)
    k.cylinder(a, n.d, deckTop(n.d) - 0.01, deckTop(n.d) + n.h, n.r, 8, brown);
  // The trackball: a brown pad flush on the deck, and the ball, whose foot
  // reaches below the deck at its front so it never floats on the slope.
  const t = C.ball;
  slab(t.a - t.pad, t.a + t.pad, t.d - t.pad, t.d + t.pad, 0.005, 0.01, brown);
  const th = deckTop(t.d) + 0.01;
  const foot = deckTop(t.d + t.r) - 0.005;
  k.lathe(
    t.a,
    t.d,
    [
      [0, foot],
      [t.r * 0.9, foot],
      [t.r, th + 0.02],
      [t.r * 0.9, th + 0.045],
      [t.r * 0.56, th + 0.06],
      [0, th + 0.065],
    ],
    12,
    s.dark,
  );
};

/** How far a small status light stands out of its face, in metres. */
const LIGHT_DEPTH = 0.01;

/**
 * The laser desk's measures, in metres: the desk (its top's depth range
 * and height, the slab's underside), the monitor and its screen, the
 * keyboard, the chair's place; the rig: the column (its place, the foot
 * plate's half sizes and height, its own half side and top), the
 * counterweight behind the shoulder, the brace under the upper arm (its
 * two ends `[d, h]`, width and half thickness), the column lights, the arm
 * (its height band and half width, the elbow's `d`, the drums' radius and
 * foot); the emitter: the barrel (foot and radius), its nose collar, the
 * fins (count, radius, thickness, lowest height, pitch), the lens radius
 * and the beam guide (radius, lowest height).
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
    plateA: 0.3,
    plateD: 0.19,
    plateH: 0.08,
    half: 0.14,
    top: 2.12,
  },
  weight: { half: 0.13, d0: -1.48, h0: 1.78 },
  brace: { foot: [-1.16, 1.5], head: [-0.6, 1.93], width: 0.06, half: 0.04 },
  lights: { h0: 1.0, pitch: 0.12, h: 0.06 },
  arm: { h0: 1.92, h1: 2.12, half: 0.1, elbowD: 0, drumR: 0.15, drumH0: 1.905 },
  barrel: { h0: 1.12, r: 0.11 },
  nose: { r: 0.14, h1: 1.2 },
  fins: { n: 6, r: 0.19, t: 0.016, h0: 1.32, pitch: 0.09 },
  lensR: 0.09,
  beam: { r: 0.008, h0: 0.52 },
} as const;

/**
 * The laser desk, standing free: `d` from -1.5 to 1.5, its front at `+d`.
 * - A terminal desk: a top slab (h 0.70 to 0.74) over `a` -1.2 to 1.2 and
 *   `d` -0.3 to 0.5 on four legs, a boxy monitor on its right (`a` 0.1 to
 *   0.7, h 0.74 to 1.2) with a dark cyan `s.glow` screen facing `+d`, and a
 *   keyboard in front of it (`d` 0.2 to 0.4). The desk top from `a` -1.1
 *   to -0.35 stays clear: it is the catalogue's surface.
 * - An empty office chair at `a` 0.3, `d` 1.0, facing the desk.
 * - The rig, in an off-white industrial grey: a heavy column on a bevelled
 *   foot plate at `a` -0.9, `d` -1.3, carrying three status lights, with a
 *   black counterweight behind its shoulder and a diagonal brace under the
 *   arm; a heavy articulated arm (h 1.92 to 2.12) from a shoulder drum
 *   forward to an elbow drum at `d` 0, then on a diagonal forearm to a
 *   wrist drum right over the chair. Everything over the clear desk top
 *   stays above h 1.9.
 * - The emitter hangs from the wrist: a long black barrel (r 0.11, h 1.12
 *   up to the wrist) with six thin cooling fins and a nose collar, a red
 *   lens at its foot looking down at the seat, and a thin red beam guide
 *   dropping from the lens to just over the seat.
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
  // The column, its lights, the counterweight and the brace.
  const rig = s.tinted(RIG_GREY);
  const black = s.tinted(LASER_BLACK);
  const C = L.column;
  const A = L.arm;
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
    rig,
  );
  const face = C.d + C.half;
  [STATUS_GREEN, STATUS_AMBER, SIGNAL_RED].forEach((tint, i) => {
    const h = L.lights.h0 + i * L.lights.pitch;
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
  const W = L.weight;
  k.bevelBox(
    C.a - W.half,
    C.a + W.half,
    W.d0,
    C.d,
    W.h0,
    A.h1,
    HOUSING_BEVEL,
    black,
  );
  const [[fd, fh], [bd, bh]] = [L.brace.foot, L.brace.head];
  profileAlong(
    kitAt,
    ORIGIN,
    tiltedBar(
      (fd + bd) / 2,
      (fh + bh) / 2,
      Math.atan2(bh - fh, bd - fd),
      Math.hypot(bd - fd, bh - fh),
      L.brace.width,
    ),
    C.a - L.brace.half,
    C.a + L.brace.half,
    rig,
  );
  // The arm: shoulder, upper arm along d, elbow, diagonal forearm, wrist.
  const drum = (a: number, d: number) =>
    k.cylinder(a, d, A.drumH0, top - 0.02, A.drumR, 12, s.dark);
  drum(C.a, C.d);
  k.bevelBox(
    C.a - A.half,
    C.a + A.half,
    C.d,
    A.elbowD,
    A.h0,
    A.h1,
    HOUSING_BEVEL,
    rig,
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
    -A.half,
    A.half,
    0,
    reach,
    A.h0,
    A.h1,
    HOUSING_BEVEL,
    rig,
  );
  drum(chairA, chairD);
  // The emitter, pointing down at the seat.
  const B = L.barrel;
  k.cylinder(chairA, chairD, B.h0, A.drumH0, B.r, 12, black);
  k.cylinder(chairA, chairD, B.h0, L.nose.h1, L.nose.r, 12, black);
  const F = L.fins;
  for (let i = 0; i < F.n; i++) {
    const h = F.h0 + i * F.pitch;
    k.cylinder(chairA, chairD, h, h + F.t, F.r, 12, s.dark);
  }
  // The lens hangs under the nose, its bezel's top on the nose's foot.
  const tube = Math.max(BEZEL_MIN, L.lensR * BEZEL_SHARE);
  const lensH = B.h0 - tube;
  lens(k, s, chairA, chairD, lensH, L.lensR, SIGNAL_RED, 0, "down");
  // The beam guide: from inside the lens down to just over the seat.
  k.cylinder(
    chairA,
    chairD,
    L.beam.h0,
    lensH,
    L.beam.r,
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
