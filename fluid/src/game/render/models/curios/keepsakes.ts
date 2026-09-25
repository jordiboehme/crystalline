/**
 * The keepsake curios' recipes (2.6b): the two collectible balls (the
 * orange one with stars inside and the red-and-white catch ball) and the
 * two under-desk curios (the trap box with its pedal and the fuel case).
 *
 * The balls are low-poly lathe spheres. `sphere` builds a whole ball
 * (the star ball's body); the catch ball's two hemispheres reuse the same
 * latitude math through the private `sphereArc`, kept apart at the
 * equator so the two colours never share a triangle (no z-fighting seam).
 * Every proud decal (a hazard stripe, the fuel case's red band and label,
 * the trefoil's sectors and disc) sits a few millimetres inside its
 * curio's catalogue box, so the body it decorates is built a little
 * short of the box on every side that carries one, leaving room for the
 * decal to stand proud without ever leaving the envelope the model tests
 * pin.
 */

import type { CurioKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import { discOutline, profileAlong, yawed } from "../common";
import { curioHalf, type CurioRecipe } from "./common";

/** Every curio in this file is built in this frame, at the origin. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

// --- Shared ball geometry (star-ball, catch-ball) -------------------------

/** Facets round a ball or its equator band, the low-poly count the brief gives the star ball. */
const BALL_SIDES = 10;
/** Latitude segments a whole ball's lathe profile takes, pole to pole. */
const BALL_BANDS = 6;

/**
 * A sphere's latitude profile from segment `i0` to `i1` of `bandsTotal`
 * pole-to-pole segments, radius `r`, its lowest point at `h0`: a full ball
 * is `i0` 0 to `bandsTotal`; a hemisphere is one half, open at the shared
 * equator (`bandsTotal / 2`) and closed at its own pole, so two
 * hemispheres built from the two halves meet with no overlap and no gap.
 */
function sphereArc(
  h0: number,
  r: number,
  bandsTotal: number,
  i0: number,
  i1: number,
): [number, number][] {
  const pts: [number, number][] = [];
  for (let i = i0; i <= i1; i++) {
    const t = -Math.PI / 2 + (Math.PI * i) / bandsTotal;
    pts.push([r * Math.cos(t), h0 + r + r * Math.sin(t)]);
  }
  return pts;
}

/**
 * A low-poly ball: `sides` facets round, `bands` latitude segments pole to
 * pole, centred at `(a, d)`, radius `r`, resting on `h0` with its lowest
 * vertex exactly there. The star ball's whole body is one call to this;
 * the catch ball's two hemispheres reuse the same latitude math through
 * `sphereArc`, so the two balls share the one technique even though the
 * catch ball never calls this directly (a call here always closes both
 * poles, which a two-colour split cannot).
 */
export function sphere(
  k: Kit,
  a: number,
  d: number,
  h0: number,
  r: number,
  sides: number,
  bands: number,
  s: Surface,
): void {
  k.lathe(a, d, sphereArc(h0, r, bands, 0, bands), sides, s);
}

// --- Star ball --------------------------------------------------------

/** The star ball's translucent-looking shell. */
const STAR_BALL_ORANGE: Rgb = [0.95, 0.55, 0.1];
/** The star ball's four inset stars. */
const STAR_RED: Rgb = [0.75, 0.1, 0.08];

/** A star's outer point radius, in metres. */
const STAR_OUTER_R = 0.007;
/** A star's inner notch radius, in metres. */
const STAR_INNER_R = 0.003;
/** How far a star stands proud of the ball's surface, in metres (its own extruded thickness). */
const STAR_THICK = 0.001;
/** The stars' azimuth either side of the ball's front, in radians (the brief's 14 degrees). */
const STAR_AZIMUTH = (14 * Math.PI) / 180;
/** The stars' height either side of the ball's centre, in metres. */
const STAR_HEIGHT_SPREAD = 0.011;

/**
 * A five-pointed star outline centred at `(cx, cy)`, alternating `outerR`
 * and `innerR` round ten points: an `extrude` outline for the star ball's
 * inset stars.
 */
function starOutline(
  cx: number,
  cy: number,
  outerR: number,
  innerR: number,
): [number, number][] {
  const points = 5;
  return Array.from({ length: points * 2 }, (_, i) => {
    const r = i % 2 === 0 ? outerR : innerR;
    const t = -Math.PI / 2 + (i * Math.PI) / points;
    return [cx + r * Math.cos(t), cy + r * Math.sin(t)];
  });
}

/**
 * The star ball: a low-poly orange sphere (`sphere`) with four small
 * five-pointed stars set into its front surface in a loose diamond, two
 * azimuths either side of dead ahead and two heights either side of the
 * ball's middle. Each star is extruded from exactly the sphere's own
 * radius at its azimuth and height, so its back face touches the ball
 * with no gap, and stands `STAR_THICK` proud of it. The stars carry a
 * steady signal light (C16's `steady` bank), so they read even in a dark
 * room; the ball's shell itself is unlit and does not glow.
 */
const starBall: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { top } = curioHalf(kind, variant);
  const r = top / 2;
  sphere(k, 0, 0, 0, r, BALL_SIDES, BALL_BANDS, s.tinted(STAR_BALL_ORANGE));
  const star = s.signal(STAR_RED);
  for (const az of [-STAR_AZIMUTH, STAR_AZIMUTH]) {
    const f = kitAt(yawed(ORIGIN, 0, 0, az));
    for (const h of [r - STAR_HEIGHT_SPREAD, r + STAR_HEIGHT_SPREAD]) {
      const dz = Math.sqrt(Math.max(0, r * r - (h - r) * (h - r)));
      f.extrude(
        starOutline(0, h, STAR_OUTER_R, STAR_INNER_R),
        dz,
        dz + STAR_THICK,
        star,
      );
    }
  }
};

// --- Catch ball ---------------------------------------------------------

/** The catch ball's glossy top shell. */
const CATCH_RED: Rgb = [0.75, 0.08, 0.06];
/** The catch ball's glossy bottom shell. */
const CATCH_WHITE: Rgb = [0.92, 0.92, 0.92];
/** The catch ball's equator band and its front ring, a plain black. */
const BAND_BLACK: Rgb = [0.05, 0.05, 0.05];
/** The catch ball's front button. */
const BUTTON_GREY: Rgb = [0.85, 0.85, 0.85];

/** The equator band's radius: a touch proud of the ball's own (about a twentieth of the diameter thick). */
const BAND_R = 0.0378;
/** Half the equator band's height (0.0038 m tall in all). */
const BAND_HALF_H = 0.0019;
/** The front ring disc's radius. */
const RING_R = 0.0095;
/** How far the ring disc reaches in from the band's own outer face, in metres (it does not stand proud of the band). */
const RING_THICK = 0.0015;
/** The button's radius, smaller than the ring it stands on. */
const BUTTON_R = 0.0075;
/** How far the button stands proud of the ring's front face, in metres. */
const BUTTON_PROUD = 0.0012;

/**
 * The catch ball: two hemispheres of `sphere`'s latitude technique
 * (`sphereArc`), red above the equator and white below, an equator band
 * a shade wider than the ball, and, on the band's front, a black ring
 * disc with a smaller white button standing proud of it. Nothing here
 * lights: the catch ball's bank is `steady` and it carries no signal or
 * blink part.
 */
const catchBall: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { top } = curioHalf(kind, variant);
  const r = top / 2;
  const half = BALL_BANDS / 2;
  k.lathe(
    0,
    0,
    sphereArc(0, r, BALL_BANDS, 0, half),
    BALL_SIDES,
    s.tinted(CATCH_WHITE),
  );
  k.lathe(
    0,
    0,
    sphereArc(0, r, BALL_BANDS, half, BALL_BANDS),
    BALL_SIDES,
    s.tinted(CATCH_RED),
  );
  k.cylinder(
    0,
    0,
    r - BAND_HALF_H,
    r + BAND_HALF_H,
    BAND_R,
    BALL_SIDES,
    s.tinted(BAND_BLACK),
  );
  // The ring and the button stand out along `d`, straight out of the
  // ball's front: a quarter turn puts `cylinderAlong`'s own axis there.
  const front = kitAt(yawed(ORIGIN, 0, 0, Math.PI / 2));
  front.cylinderAlong(
    BAND_R - RING_THICK,
    BAND_R,
    0,
    r,
    RING_R,
    8,
    s.tinted(BAND_BLACK),
  );
  front.cylinderAlong(
    BAND_R,
    BAND_R + BUTTON_PROUD,
    0,
    r,
    BUTTON_R,
    8,
    s.tinted(BUTTON_GREY),
  );
};

// --- Trap box and pedal ---------------------------------------------------

/** The trap's body and its cable: a plain black. */
const TRAP_BODY_BLACK: Rgb = [0.05, 0.05, 0.05];
/** The trap's lid, its hinge rods and its handle: brushed silver. */
const LID_SILVER: Rgb = [0.55, 0.55, 0.57];
/** The two tubes along the trap's back. */
const TUBES_RED: Rgb = [0.7, 0.1, 0.1];
/** The front bar's three amber segments. */
const BAR_AMBER: Rgb = [1.0, 0.6, 0.12];
/** The rear beacon light. */
const BEACON_RED: Rgb = [0.85, 0.15, 0.1];

/** The trap body's own bevel. */
const TRAP_BEVEL = 0.008;
/** The trap's `+a` edge: the pedal and its cable sit past it. */
const TRAP_A1 = 0.06;
/** The trap's depth, `d0` to `d1` (its two long sides). */
const TRAP_D0 = -0.1;
const TRAP_D1 = 0.1;
/** The trap body's own height. */
const TRAP_H1 = 0.15;
/** The lid's height, split across the middle (C: the split runs along `d`, hinged on the two outer long edges). */
const LID_H0 = 0.15;
const LID_H1 = 0.162;
/** How far the hazard stripe panel's inset from the trap's ends, in metres. */
const HAZARD_INSET = 0.03;
/** The hazard stripe panels' height range. */
const HAZARD_H0 = 0.02;
const HAZARD_H1 = 0.11;
/** A hinge rod's radius. */
const HINGE_R = 0.006;
/** How far a small light stands proud of the trap's front or back face. */
const LIGHT_PROUD = 0.006;
/** The front bar and the rear beacon's height range. */
const LIGHT_H0 = 0.12;
const LIGHT_H1 = 0.135;
/** The three front bar segments' `a` centres. */
const BAR_A = [-0.2, -0.12, -0.04];
/** Half a bar segment's width along `a`. */
const BAR_HALF = 0.02;
/** The rear beacon's `a` centre. */
const BEACON_A = -0.12;
/** Half the rear beacon's width along `a` (a little wider than a bar segment, the "dome"). */
const BEACON_HALF = 0.03;
/** The handle's half span along `a`, and its post and bar radii. */
const HANDLE_HALF = 0.05;
const HANDLE_POST_R = 0.004;
const HANDLE_BAR_R = 0.006;
/** The two side tubes' radius, their `d` position and their two heights. */
const TUBE_R = 0.008;
const TUBE_D = -0.13;
const TUBE_H = [0.05, 0.09];
/** The pedal's box, in metres: its `a` and `d` range and its wedge heights. */
const PEDAL_A0 = 0.16;
const PEDAL_A1 = 0.28;
const PEDAL_D0 = -0.07;
const PEDAL_D1 = 0.07;
const PEDAL_LOW = 0.02;
const PEDAL_HIGH = 0.05;
/** The pedal's silver top pad, inset from the wedge's own plateau. */
const PAD_INSET = 0.015;
const PAD_THICK = 0.003;
/** The cable's segments between the trap and the pedal: its count, its height and its half width. */
const CABLE_SEGMENTS = 4;
const CABLE_H0 = 0.002;
const CABLE_H1 = 0.006;
const CABLE_HALF_D = 0.006;

/**
 * The trap box, low and black with a two-half hinged lid, hazard-striped
 * long sides, a front bar and a rear beacon (C16's `status` bank, each
 * its own strobe group), and, apart from it across a bare strip of
 * floor, its foot pedal joined by a short run of cable. Built centred on
 * the curio's own box (`curioHalf`); the trap sits toward `-a`, the pedal
 * toward `+a`, with the gap between them where the cable runs.
 */
const trapBox: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, top } = curioHalf(kind, variant);
  const trapA0 = -hw;
  k.bevelBox(
    trapA0,
    TRAP_A1,
    TRAP_D0,
    TRAP_D1,
    0,
    TRAP_H1,
    TRAP_BEVEL,
    s.tinted(TRAP_BODY_BLACK),
  );
  const lidA0 = trapA0 + HAZARD_INSET;
  const lidA1 = TRAP_A1 - HAZARD_INSET;
  const silver = s.tinted(LID_SILVER);
  k.box(lidA0, lidA1, TRAP_D0, 0, LID_H0, LID_H1, silver);
  k.box(lidA0, lidA1, 0, TRAP_D1, LID_H0, LID_H1, silver);
  for (const d of [TRAP_D0, TRAP_D1])
    k.cylinderAlong(lidA0, lidA1, d, LID_H1, HINGE_R, 6, silver);
  // The handle: two short posts and a bar across the lid's middle, its
  // bar reaching exactly the curio's own top.
  const handleAxis = top - HANDLE_BAR_R;
  for (const a of [-HANDLE_HALF, HANDLE_HALF])
    k.cylinder(a, 0, LID_H1, handleAxis, HANDLE_POST_R, 6, silver);
  k.cylinderAlong(
    -HANDLE_HALF,
    HANDLE_HALF,
    0,
    handleAxis,
    HANDLE_BAR_R,
    6,
    silver,
  );
  // The hazard stripe panels on both long sides.
  const hazardA0 = trapA0 + HAZARD_INSET;
  const hazardA1 = TRAP_A1 - HAZARD_INSET;
  k.box(
    hazardA0,
    hazardA1,
    TRAP_D0 - DECAL_LIFT,
    TRAP_D0,
    HAZARD_H0,
    HAZARD_H1,
    s.hazard,
  );
  k.box(
    hazardA0,
    hazardA1,
    TRAP_D1,
    TRAP_D1 + DECAL_LIFT,
    HAZARD_H0,
    HAZARD_H1,
    s.hazard,
  );
  // Two thin red tubes along the back.
  for (const h of TUBE_H)
    k.cylinderAlong(
      hazardA0,
      hazardA1,
      TUBE_D,
      h,
      TUBE_R,
      8,
      s.tinted(TUBES_RED),
    );
  // The front bar (three amber segments, groups 0 to 2) and the rear beacon (group 3).
  for (const [i, a] of BAR_A.entries())
    k.box(
      a - BAR_HALF,
      a + BAR_HALF,
      TRAP_D1,
      TRAP_D1 + LIGHT_PROUD,
      LIGHT_H0,
      LIGHT_H1,
      s.blink(BAR_AMBER, i),
    );
  k.box(
    BEACON_A - BEACON_HALF,
    BEACON_A + BEACON_HALF,
    TRAP_D0 - LIGHT_PROUD,
    TRAP_D0,
    LIGHT_H0,
    LIGHT_H1,
    s.blink(BEACON_RED, 3),
  );
  // The pedal: a black wedge with a silver top pad, apart from the trap.
  const wedge: readonly (readonly [d: number, h: number])[] = [
    [PEDAL_D0, 0],
    [PEDAL_D0, PEDAL_LOW],
    [PEDAL_D0 + PAD_INSET, PEDAL_HIGH],
    [PEDAL_D1 - PAD_INSET, PEDAL_HIGH],
    [PEDAL_D1, PEDAL_LOW],
    [PEDAL_D1, 0],
  ];
  profileAlong(
    kitAt,
    ORIGIN,
    wedge,
    PEDAL_A0,
    PEDAL_A1,
    s.tinted(TRAP_BODY_BLACK),
  );
  k.box(
    PEDAL_A0 + PAD_INSET,
    PEDAL_A1 - PAD_INSET,
    PEDAL_D0 + PAD_INSET,
    PEDAL_D1 - PAD_INSET,
    PEDAL_HIGH,
    PEDAL_HIGH + PAD_THICK,
    silver,
  );
  // The cable: a few short segments lying on the floor between the two.
  const span = PEDAL_A0 - TRAP_A1;
  const step = span / CABLE_SEGMENTS;
  const cable = s.tinted(TRAP_BODY_BLACK);
  for (let i = 0; i < CABLE_SEGMENTS; i++)
    k.box(
      TRAP_A1 + step * i,
      TRAP_A1 + step * (i + 1),
      -CABLE_HALF_D,
      CABLE_HALF_D,
      CABLE_H0,
      CABLE_H1,
      cable,
    );
};

// --- Fuel case ------------------------------------------------------------

/** The case's own aluminium shell. */
const CASE_GREY: Rgb = [0.62, 0.63, 0.65];
/** Every piece of steel hardware: the corner guards, the lid seam, the latches and the handle. */
const STEEL_GREY: Rgb = [0.45, 0.46, 0.48];
/** The red stripe band. */
const STRIPE_RED: Rgb = [0.75, 0.1, 0.08];
/** The label's backing plate. */
const LABEL_YELLOW: Rgb = [0.95, 0.78, 0.05];
/** The trefoil, and the case's corner guards and lid seam. */
const TREFOIL_BLACK: Rgb = [0.05, 0.05, 0.05];

/** How far the case body is built inside its catalogue box on `a` and `d`, leaving room for every proud detail. */
const CASE_MARGIN = 0.02;
/** How much of the catalogue box's height is left above the case body for the handle. */
const CASE_HANDLE_ALLOW = 0.04;
/** The case body's own bevel. */
const CASE_BEVEL = 0.01;

/** A corner guard's size along each of `a` and `d`, and how far it stands proud past the case body. */
const GUARD_SIZE = 0.03;
const GUARD_PROUD = 0.006;
/** The red stripe's and the lid seam's height ranges. */
const STRIPE_H0 = 0.07;
const STRIPE_H1 = 0.1;
const SEAM_H0 = 0.15;
const SEAM_H1 = 0.157;
/** A latch's half width, its own depth and its lever's further depth. */
const LATCH_HALF = 0.015;
const LATCH_DEPTH = 0.012;
const LEVER_DEPTH = 0.006;
const LATCH_H0 = 0.11;
const LATCH_H1 = 0.145;
const LATCH_A = [-0.06, 0.06];
/** The handle's two posts and its bar, all past the case body's own top. */
const HANDLE_POST_A = 0.05;
const HANDLE_POST_R2 = 0.006;
const HANDLE_BAR_R2 = 0.008;
/** The yellow label: a square, its own thickness and the trefoil's extra thickness on top of it. */
const LABEL_SIDE = 0.09;
const LABEL_H0 = 0.11;
const LABEL_THICK = 0.001;
const TREFOIL_THICK = 0.001;
/** The trefoil's inner and outer radius, and its centre disc's radius. */
const TREFOIL_R0 = 0.008;
const TREFOIL_R1 = 0.035;
const TREFOIL_CENTRE_R = 0.006;
/** The trefoil's three sectors' centre angles, in degrees: one straight up, two down at 120 degrees either side. */
const TREFOIL_ANGLES = [90, 210, 330];
/** Half a sector's angular width, in degrees. */
const TREFOIL_HALF_DEG = 30;

/**
 * An annular sector outline from `r0` to `r1`, spanning `centreDeg` plus
 * or minus `halfDeg`, its arcs walked in a few segments: one of the
 * trefoil's three blades. Angles are degrees, standard turns (0 along
 * `+a`, 90 along `+h`).
 */
function sectorOutline(
  cx: number,
  cy: number,
  r0: number,
  r1: number,
  centreDeg: number,
  halfDeg: number,
): [number, number][] {
  const segs = 3;
  const at = (r: number, deg: number): [number, number] => {
    const t = (deg * Math.PI) / 180;
    return [cx + r * Math.cos(t), cy + r * Math.sin(t)];
  };
  const outer = Array.from({ length: segs + 1 }, (_, i) =>
    at(r1, centreDeg - halfDeg + (2 * halfDeg * i) / segs),
  );
  const inner = Array.from({ length: segs + 1 }, (_, i) =>
    at(r0, centreDeg + halfDeg - (2 * halfDeg * i) / segs),
  );
  return [...outer, ...inner];
}

/**
 * The fuel case: a small aluminium hard case with steel corner guards, a
 * dark lid seam, a red stripe wrapped round all four faces, two latches
 * and a carry handle, and, on the front above the stripe, a yellow label
 * carrying a black radiation trefoil (three sectors and a centre disc,
 * each extruded proud of the label). No lights, no text: the case's bank
 * is steady.
 */
const fuelCase: CurioRecipe = ({ k, s, variant, kind }) => {
  const { hw, hd, top } = curioHalf(kind, variant);
  const bw = hw - CASE_MARGIN;
  const bd = hd - CASE_MARGIN;
  const bh = top - CASE_HANDLE_ALLOW;
  k.bevelBox(-bw, bw, -bd, bd, 0, bh, CASE_BEVEL, s.tinted(CASE_GREY));

  const steel = s.tinted(STEEL_GREY);
  for (const sa of [-1, 1] as const)
    for (const sd of [-1, 1] as const)
      for (const sh of [0, 1] as const) {
        const a0 = sa > 0 ? bw - GUARD_SIZE : -bw - GUARD_PROUD;
        const a1 = sa > 0 ? bw + GUARD_PROUD : -bw + GUARD_SIZE;
        const d0 = sd > 0 ? bd - GUARD_SIZE : -bd - GUARD_PROUD;
        const d1 = sd > 0 ? bd + GUARD_PROUD : -bd + GUARD_SIZE;
        const h0 = sh > 0 ? bh - GUARD_SIZE : 0;
        const h1 = sh > 0 ? bh : GUARD_SIZE;
        k.box(a0, a1, d0, d1, h0, h1, steel);
      }

  // A band wrapped round all four faces, shared by the lid seam and the red stripe.
  const band = (h0: number, h1: number, tint: Surface) => {
    k.box(-bw, bw, bd, bd + DECAL_LIFT, h0, h1, tint);
    k.box(-bw, bw, -bd - DECAL_LIFT, -bd, h0, h1, tint);
    k.box(bw, bw + DECAL_LIFT, -bd, bd, h0, h1, tint);
    k.box(-bw - DECAL_LIFT, -bw, -bd, bd, h0, h1, tint);
  };
  band(SEAM_H0, SEAM_H1, steel);
  band(STRIPE_H0, STRIPE_H1, s.tinted(STRIPE_RED));

  // Two latches on the front face, each with a small lever.
  for (const a of LATCH_A) {
    k.box(
      a - LATCH_HALF,
      a + LATCH_HALF,
      bd,
      bd + LATCH_DEPTH,
      LATCH_H0,
      LATCH_H1,
      steel,
    );
    k.box(
      a - LATCH_HALF * 0.6,
      a + LATCH_HALF * 0.6,
      bd + LATCH_DEPTH,
      bd + LATCH_DEPTH + LEVER_DEPTH,
      LATCH_H0 + 0.008,
      LATCH_H0 + 0.02,
      steel,
    );
  }

  // A carry handle on top: two posts and a grip bar, its bar reaching
  // exactly the curio's own top.
  const postTop = top - HANDLE_BAR_R2;
  for (const a of [-HANDLE_POST_A, HANDLE_POST_A])
    k.cylinder(a, 0, bh, postTop, HANDLE_POST_R2, 8, steel);
  k.cylinderAlong(
    -HANDLE_POST_A,
    HANDLE_POST_A,
    0,
    postTop,
    HANDLE_BAR_R2,
    8,
    steel,
  );

  // The yellow label on the front, above the stripe, and its trefoil.
  const labelHalf = LABEL_SIDE / 2;
  const labelD0 = bd + DECAL_LIFT;
  k.box(
    -labelHalf,
    labelHalf,
    labelD0,
    labelD0 + LABEL_THICK,
    LABEL_H0,
    LABEL_H0 + LABEL_SIDE,
    s.tinted(LABEL_YELLOW),
  );
  const trefoilD0 = labelD0 + LABEL_THICK;
  const trefoil = s.tinted(TREFOIL_BLACK);
  const cy = LABEL_H0 + labelHalf;
  for (const deg of TREFOIL_ANGLES)
    k.extrude(
      sectorOutline(0, cy, TREFOIL_R0, TREFOIL_R1, deg, TREFOIL_HALF_DEG),
      trefoilD0,
      trefoilD0 + TREFOIL_THICK,
      trefoil,
    );
  k.extrude(
    discOutline(0, cy, TREFOIL_CENTRE_R, 10),
    trefoilD0,
    trefoilD0 + TREFOIL_THICK,
    trefoil,
  );
};

/** The keepsake kinds' recipes, one per kind. */
export const KEEPSAKE_RECIPES = {
  "star-ball": starBall,
  "catch-ball": catchBall,
  "trap-box": trapBox,
  "fuel-case": fuelCase,
} satisfies Record<
  Extract<CurioKind, "star-ball" | "catch-ball" | "trap-box" | "fuel-case">,
  CurioRecipe
>;
