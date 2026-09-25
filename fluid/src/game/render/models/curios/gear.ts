/**
 * The gear curios' recipes (2.6b): the light sword (lying in its cradle,
 * or lit upright in its stand in blue or green), the green pistol, the pink
 * gadget (alone or in a cluster) and the winged meter. Colours and helpers
 * stay in this file, which imports only `common.ts` of the curio batches.
 *
 * Each follows its original's shape closely and copies no name or marking:
 * - The sword's hilt is a long thin metal tube with a flared emitter and
 *   two clip tabs at its top, a small glass eye below them, a flat clamp
 *   band round the upper third carrying a round red button, seven black
 *   grip ribs round the lower half and a bottom cap with a small D-ring
 *   (`hilt`). Lying, it rests in a low dark cradle whose two forks rise to
 *   the curio's top; upright, it stands pommel down in a round dark stand
 *   with its lit blade standing on the emitter. The blade and its rounded
 *   tip are one `lathe`, so the whole blade touches the emitter (the glow
 *   check's host).
 * - The pistol is a grey-white handheld with a darker grip angled down and
 *   back, a short front nozzle with a dark flared tip, a knob on its side,
 *   and a bulbous chamber of glowing green fluid on its top in a grey
 *   collar; a thin strut under the nozzle lets it stand.
 * - The gadget is an asymmetric pink blob on a flat oval base: a lower
 *   mass, a pinched waist wrapped by a mauve band, a top bulb set off the
 *   axis, a knobby stalk, two tendrils with juice-tinted tips hanging off
 *   one side and a small grey handle out of the other (`gadget`); the
 *   cluster is three of them in a loose row.
 * - The meter is a black box on a short grip and a flared foot, with a
 *   faceplate holding a small green readout and two tiny knobs, and two
 *   silver wings hinged near its upper corners, fanned out and up, each
 *   carrying amber lights on its front face.
 *
 * Blinking follows the curio bank rule (C16): the sword's blade (and, in
 * the cradle, its glass eye) and the pistol's chamber are group 0 of the
 * `breathe` bank, so they pulse slowly; the meter's seven wing lights take
 * the groups that make the `chase` bank visit them in the jumping order
 * `METER.chase`, with group 7 left as the dark beat. The gadget and the
 * meter's readout do not blink.
 */

import type { CurioKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { frameAt, type Frame, type Kit } from "../../kit";
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
import { curioHalf, type CurioRecipe } from "./common";

/** Every recipe's main frame: at the origin, facing north (`frameAt([0, 0, 0], 0)`). */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/** A quarter turn, in radians: a `yawed` frame by this runs its `along` down the old `inward`. */
const QUARTER = Math.PI / 2;

/** The hilt's tube, cap and emitter: a worn brushed silver-grey metal. */
const HILT_METAL: Rgb = [0.62, 0.62, 0.63];

/** The hilt's clamp band: a darker steel than the tube. */
const CLAMP_STEEL: Rgb = [0.45, 0.45, 0.47];

/** The hilt's seven grip ribs: a black plastic. */
const RIB_BLACK: Rgb = [0.05, 0.05, 0.05];

/** The round push-button on the clamp: a bright red. */
const BUTTON_RED: Rgb = [0.75, 0.08, 0.06];

/** The glass eye's bezel: a clear silver. */
const EYE_BEZEL: Rgb = [0.8, 0.8, 0.8];

/** The glass eye's centre when it is not lit: a dark lens. */
const EYE_CENTRE: Rgb = [0.1, 0.1, 0.1];

/** The glass eye's faint glow while the sword lies in its cradle: a pale cold blue. */
const EYE_GLOW: Rgb = [0.7, 0.8, 1.0];

/** The cradle and the stand the sword rests in: a dark near-black grey. */
const STAND_DARK: Rgb = [0.18, 0.18, 0.2];

/** The blue sword's blade (variant 1). */
const BLADE_BLUE: Rgb = [0.35, 0.6, 1.0];

/** The green sword's blade (variant 2). */
const BLADE_GREEN: Rgb = [0.4, 1.0, 0.45];

/** The pistol's body and knob cap: a grey-white plastic. */
const PISTOL_BODY: Rgb = [0.85, 0.86, 0.84];

/** The pistol's grip and the chamber's collar: a dark grey. */
const PISTOL_GRIP: Rgb = [0.3, 0.31, 0.33];

/** The pistol's flared nozzle tip: a near black trim. */
const PISTOL_TRIM: Rgb = [0.12, 0.12, 0.13];

/** The pistol's side knob: a black plastic. */
const PISTOL_KNOB: Rgb = [0.08, 0.08, 0.08];

/** The pistol's chamber: a bright glowing green fluid. */
const FLUID_GREEN: Rgb = [0.25, 0.95, 0.25];

/** The gadget's body: a soft pink flesh tone. */
const GADGET_PINK: Rgb = [0.85, 0.55, 0.58];

/** The gadget's folds, base and waist band: a darker mauve pink. */
const GADGET_FOLD: Rgb = [0.65, 0.35, 0.4];

/** The tips of the gadget's tendrils: a pale yellowish juice tint. */
const GADGET_JUICE: Rgb = [0.88, 0.7, 0.55];

/** The gadget's small handle: a plain grey. */
const GADGET_HANDLE: Rgb = [0.55, 0.55, 0.57];

/** The meter's body, display bezel and knobs: a matte black. */
const METER_BLACK: Rgb = [0.06, 0.06, 0.07];

/** The meter's grip, foot and faceplate: a dark grey. */
const METER_GRIP: Rgb = [0.18, 0.18, 0.2];

/** The meter's wings and hinge knuckles: a metallic silver. */
const WING_SILVER: Rgb = [0.6, 0.6, 0.62];

/** The meter's wing lights: an amber. */
const WING_AMBER: Rgb = [1.0, 0.6, 0.12];

/** The meter's small readout: a green glow. */
const READOUT_GREEN: Rgb = [0.1, 0.8, 0.3];

/**
 * The sword's hilt, in metres, every distance along it (`e`) measured from
 * the emitter end: its whole length (emitter and cap included), the tube's
 * radius and facets; the emitter (its length, its radius, and the flare's
 * start, where it steps out from the tube); the two clip tabs past the
 * emitter's rim (how far past, their half width and how far in from the
 * rim they sit); the glass eye (where, its bezel's and centre's radii, how
 * far the centre stands out); the clamp band (from and to, its half
 * section); the red button (radius, how far it stands proud of the clamp);
 * the seven ribs (their start, length, section across the tube and how
 * far they stand proud, each sunk 1 mm into the tube's facets); the bottom
 * cap (length, radius) and the D-ring (radius to the tube's middle and the
 * tube's radius). Exported so the sword's test can find ribs, clamp and
 * button by their numbers.
 */
export const HILT = {
  length: 0.27,
  r: 0.0175,
  sides: 10,
  emitter: { length: 0.03, r: 0.021, flare: 0.018 },
  tab: { out: 0.006, half: 0.003, inset: 0.004 },
  eye: { e: 0.05, bezel: 0.0055, centre: 0.0035, out: 0.0015 },
  clamp: [0.07, 0.11],
  clampHalf: 0.0205,
  button: { r: 0.004, proud: 0.005 },
  rib: { e0: 0.155, length: 0.092, width: 0.006, proud: 0.004, sink: 0.001 },
  cap: { length: 0.012, r: 0.019 },
  dRing: { radius: 0.0055, tube: 0.0015 },
} as const;

/**
 * The sword's cradle and stand, in metres: the lying hilt's axis height;
 * the cradle's base plate (half length, half width, height); its two
 * forks, each a U standing on the plate (where along `a` each stands,
 * its half thickness, the top of the U's floor just under the tube, the
 * prongs' inner and outer `d`; the prongs rise to the curio's top); the upright stand's disc
 * height, its collar (radius and top) and where the upright hilt's pommel
 * stands (`standBase`); the blade's radius and facets, and how far under
 * the top its rounded tip starts.
 */
export const SWORD = {
  lyingAxis: 0.045,
  plate: { hw: 0.14, hd: 0.04, h: 0.01 },
  forks: [-0.129, 0.09],
  fork: { half: 0.004, cross: 0.026, prong: [0.021, 0.025] },
  disc: 0.02,
  collar: { r: 0.025, top: 0.045 },
  standBase: 0.03,
  blade: { r: 0.012, sides: 8, tip: 0.03 },
} as const;

/**
 * Builds the sword's hilt: `lying` along `a` with its axis at height
 * `base`, the emitter towards `+a`, centred on `a` 0; or upright on the
 * vertical axis from its pommel at `base`, the emitter up, its eye and
 * button facing `+d`. `eye` is the glass eye centre's surface: a faint
 * blink in the cradle, the plain dark lens on the lit sword.
 *
 * From the emitter end: the flared emitter (two steps lying, a `lathe`
 * upright) with two small clip tabs past its rim, the glass eye (a bezel
 * disc with its centre standing out of it, facing up when lying and out
 * of the front when upright), the flattened clamp band with the round red
 * button on its outer face (the top when lying, the front upright), the
 * seven black grip ribs spaced evenly round the lower half (lying: each a
 * `profileAlong` of a small rotated rectangle; upright: a box in a
 * `yawed` frame), and the bottom cap with its small D-ring hanging off
 * the end. Upright, the stand's collar wraps the pommel, so the D-ring is
 * left out there: it would sit hidden inside the collar.
 */
export function hilt(
  k: Kit,
  kitAt: KitAt,
  s: Surfaces,
  lying: boolean,
  base: number,
  eye: Surface,
): void {
  const H = HILT;
  const L = H.length;
  const metal = s.tinted(HILT_METAL, LAYER.metal);
  const steel = s.tinted(CLAMP_STEEL, LAYER.metal);
  const rib = s.tinted(RIB_BLACK);
  const red = s.tinted(BUTTON_RED);
  const bezel = s.tinted(EYE_BEZEL, LAYER.metal);
  const E = H.emitter;
  const [c0, c1] = H.clamp;
  const ch = H.clampHalf;
  const rib0 = H.rib.e0;
  const rib1 = H.rib.e0 + H.rib.length;
  const rc = H.r - H.rib.sink + (H.rib.proud + H.rib.sink) / 2;
  const ribRadial = H.rib.proud + H.rib.sink;
  const ribAngle = (i: number) => QUARTER + (2 * Math.PI * i) / 7;
  if (lying) {
    const x = (e: number) => L / 2 - e;
    k.cylinderAlong(
      x(L - H.cap.length),
      x(E.length),
      0,
      base,
      H.r,
      H.sides,
      metal,
    );
    k.cylinderAlong(
      x(E.length),
      x(E.flare),
      0,
      base,
      (H.r + E.r) / 2,
      H.sides,
      metal,
    );
    k.cylinderAlong(x(E.flare), x(0), 0, base, E.r, H.sides, metal);
    for (const side of [-1, 1]) {
      const d0 = side * (E.r - H.tab.inset);
      const d1 = side * (E.r - H.tab.inset / 4);
      k.box(
        x(0) - 0.001,
        x(0) + H.tab.out,
        d0,
        d1,
        base - H.tab.half,
        base + H.tab.half,
        metal,
      );
    }
    const top = base + H.r;
    k.cylinder(
      x(H.eye.e),
      0,
      top - 0.002,
      top + H.eye.out,
      H.eye.bezel,
      6,
      bezel,
    );
    k.cylinder(
      x(H.eye.e),
      0,
      top - 0.001,
      top + 2 * H.eye.out,
      H.eye.centre,
      6,
      eye,
    );
    k.box(x(c1), x(c0), -ch, ch, base - ch, base + ch, steel);
    k.cylinder(
      x((c0 + c1) / 2),
      0,
      base + ch - 0.0005,
      base + ch + H.button.proud,
      H.button.r,
      6,
      red,
    );
    for (let i = 0; i < 7; i++) {
      const t = ribAngle(i);
      profileAlong(
        kitAt,
        ORIGIN,
        tiltedBar(
          rc * Math.cos(t),
          base + rc * Math.sin(t),
          t,
          ribRadial,
          H.rib.width,
        ),
        x(rib1),
        x(rib0),
        rib,
      );
    }
    k.cylinderAlong(
      x(L),
      x(L - H.cap.length),
      0,
      base,
      H.cap.r,
      H.sides,
      metal,
    );
    const R = H.dRing;
    k.ring(
      x(L) - R.radius + R.tube,
      0,
      base,
      R.radius,
      R.tube,
      3,
      6,
      steel,
      "inward",
    );
    return;
  }
  const y = (e: number) => base + L - e;
  k.cylinder(0, 0, y(L - H.cap.length), y(E.length), H.r, H.sides, metal);
  k.lathe(
    0,
    0,
    [
      [H.r * 0.95, y(E.length)],
      [(H.r + E.r) / 2, y(E.length)],
      [E.r, y(E.flare)],
      [E.r, y(0)],
      [0, y(0)],
    ],
    H.sides,
    metal,
  );
  for (const side of [-1, 1]) {
    const a0 = side * (E.r - H.tab.inset);
    const a1 = side * (E.r - H.tab.inset / 4);
    k.box(
      a0,
      a1,
      -H.tab.half,
      H.tab.half,
      y(0) - 0.001,
      y(0) + H.tab.out,
      metal,
    );
  }
  const front = H.r;
  k.extrude(
    discOutline(0, y(H.eye.e), H.eye.bezel, 6),
    front - 0.002,
    front + H.eye.out,
    bezel,
  );
  k.extrude(
    discOutline(0, y(H.eye.e), H.eye.centre, 6),
    front - 0.001,
    front + 2 * H.eye.out,
    eye,
  );
  k.box(-ch, ch, -ch, ch, y(c1), y(c0), steel);
  kitAt(yawed(ORIGIN, 0, 0, QUARTER)).cylinderAlong(
    ch - 0.0005,
    ch + H.button.proud,
    0,
    y((c0 + c1) / 2),
    H.button.r,
    6,
    red,
  );
  for (let i = 0; i < 7; i++) {
    kitAt(yawed(ORIGIN, 0, 0, ribAngle(i))).box(
      -H.rib.width / 2,
      H.rib.width / 2,
      H.r - H.rib.sink,
      H.r + H.rib.proud,
      y(rib1),
      y(rib0),
      rib,
    );
  }
  k.cylinder(0, 0, y(L), y(L - H.cap.length), H.cap.r, H.sides, metal);
}

/**
 * The light sword. Variant 0 lies in a low dark cradle: a base plate and
 * two forks whose cross bars hold the hilt from below and whose prongs
 * rise either side of it to the curio's top, with the glass eye glowing
 * faint (blink group 0). Variants 1 (blue) and 2 (green) stand upright in
 * a round dark stand, pommel down in its collar, the blade on the emitter
 * up to the top: one `lathe`, a rod with a rounded tip, open at its foot
 * where the emitter's top closes it, breathing in blink group 0 like a
 * slow hum. The eye stays a plain dark lens there.
 */
const lightSword: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, top } = curioHalf(kind, variant);
  const S = SWORD;
  const dark = s.tinted(STAND_DARK);
  if (variant === 0) {
    const P = S.plate;
    k.box(-P.hw, P.hw, -P.hd, P.hd, 0, P.h, dark);
    const F = S.fork;
    const [p0, p1] = F.prong;
    const fork: [number, number][] = [
      [-p1, P.h],
      [p1, P.h],
      [p1, top],
      [p0, top],
      [p0, F.cross],
      [-p0, F.cross],
      [-p0, top],
      [-p1, top],
    ];
    for (const a of S.forks)
      profileAlong(kitAt, ORIGIN, fork, a - F.half, a + F.half, dark);
    hilt(k, kitAt, s, true, S.lyingAxis, s.blink(EYE_GLOW, 0));
    return;
  }
  k.cylinder(0, 0, 0, S.disc, hw, 10, dark);
  k.cylinder(0, 0, S.disc, S.collar.top, S.collar.r, 8, dark);
  hilt(k, kitAt, s, false, S.standBase, s.tinted(EYE_CENTRE));
  const B = S.blade;
  const from = S.standBase + HILT.length;
  const tip = top - B.tip;
  k.lathe(
    0,
    0,
    [
      [B.r, from],
      [B.r, tip],
      [B.r * 0.7, tip + B.tip * 0.6],
      [0, top],
    ],
    B.sides,
    s.blink(variant === 1 ? BLADE_BLUE : BLADE_GREEN, 0),
  );
};

/**
 * The pistol's measures, in metres, its long axis along `a` and the
 * nozzle towards `+a`: the body (`a`, half depth, `h`, bevel); the grip
 * (its centre along `a`, its lean from `+a`, length, width and half depth;
 * it is lifted so its lowest corner stands at `h` 0); the front strut
 * (`a`, half depth); the nozzle (`a`, axis height, radius) and its flared
 * tip (`a`, radius); the chamber (its centre along `a`, the collar's
 * radius and tube, and the bulb's profile as `[r, h]` from the body's top
 * up to the curio's top, closed on the axis); the side knob (along `a`,
 * height, radius, how far out; its cap's radius and length).
 */
export const PISTOL = {
  body: { a: [-0.11, 0.07], hd: 0.035, h: [0.06, 0.12], bevel: 0.008 },
  grip: { a: -0.075, lean: Math.PI / 3, length: 0.09, width: 0.035, hd: 0.022 },
  strut: { a: [0.084, 0.094], hd: 0.006 },
  nozzle: { a: [0.07, 0.11], h: 0.09, r: 0.022 },
  tip: { a: [0.104, 0.125], r: 0.026 },
  chamber: {
    a: -0.035,
    collar: { radius: 0.021, tube: 0.004 },
    bulb: [
      [0.018, 0],
      [0.03, 0.008],
      [0.035, 0.022],
      [0.032, 0.036],
      [0.02, 0.047],
    ],
  },
  knob: { a: -0.07, h: 0.09, r: 0.011, out: 0.008, cap: 0.007, capOut: 0.004 },
} as const;

/**
 * The green pistol: the bevelled grey-white body, the darker grip under
 * its rear leaning down and back to the surface, a thin strut from under
 * the nozzle down to the surface so it stands, the short nozzle with its
 * dark flared tip, the black knob on its `+d` side (a cylinder in a
 * `yawed` quarter frame, so it runs along `d`) with a lighter cap, and on
 * the body's top a grey collar holding the bulbous chamber of green fluid
 * that breathes in blink group 0 and is the highest part.
 */
const greenPistol: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { top } = curioHalf(kind, variant);
  const P = PISTOL;
  const body = s.tinted(PISTOL_BODY);
  const grip = s.tinted(PISTOL_GRIP);
  const [b0, b1] = P.body.h;
  k.bevelBox(
    P.body.a[0],
    P.body.a[1],
    -P.body.hd,
    P.body.hd,
    b0,
    b1,
    P.body.bevel,
    body,
  );
  const G = P.grip;
  const lift =
    (G.length / 2) * Math.sin(G.lean) + (G.width / 2) * Math.cos(G.lean);
  k.extrude(tiltedBar(G.a, lift, G.lean, G.length, G.width), -G.hd, G.hd, grip);
  const N = P.nozzle;
  k.box(
    P.strut.a[0],
    P.strut.a[1],
    -P.strut.hd,
    P.strut.hd,
    0,
    N.h - N.r + 0.002,
    grip,
  );
  k.cylinderAlong(N.a[0] - 0.002, N.a[1], 0, N.h, N.r, 10, body);
  k.cylinderAlong(
    P.tip.a[0],
    P.tip.a[1],
    0,
    N.h,
    P.tip.r,
    10,
    s.tinted(PISTOL_TRIM),
  );
  const C = P.chamber;
  k.ring(C.a, 0, b1 + 0.002, C.collar.radius, C.collar.tube, 3, 10, grip, "up");
  k.lathe(
    C.a,
    0,
    [[0, b1], ...C.bulb.map(([r, h]) => [r, b1 + h] as const), [0, top]],
    10,
    s.blink(FLUID_GREEN, 0),
  );
  const K = P.knob;
  const side = kitAt(yawed(ORIGIN, 0, 0, QUARTER));
  side.cylinderAlong(
    P.body.hd - 0.001,
    P.body.hd + K.out,
    -K.a,
    K.h,
    K.r,
    8,
    s.tinted(PISTOL_KNOB),
  );
  side.cylinderAlong(
    P.body.hd + K.out,
    P.body.hd + K.out + K.capOut,
    -K.a,
    K.h,
    K.cap,
    8,
    body,
  );
};

/**
 * One gadget's measures at scale 1, in metres, centred on its base: the
 * flat oval base (the half length of its middle box, its half width,
 * which is also the end cylinders' radius, and its height); the lower
 * mass and pinched waist as one profile `[r, h]` from the base's top; the
 * mauve band round the waist (height, radius to its tube's middle, tube);
 * the top bulb (its offset along `a` off the base's axis, its foot `h0`,
 * its profile); the stalk (its foot and radius there; its tip is the
 * curio's top); the knobs (`[a, d, h0, h1, r]`, `a` from the base's
 * axis); the two tendrils (`[root a, root h, tip a, tip h, d]`, their tip
 * share in juice, width and half thickness); and the handle (`a` range,
 * axis height, radius).
 */
export const GADGET = {
  base: { half: 0.02, hd: 0.03, h: 0.015 },
  lower: [
    [0, 0.015],
    [0.036, 0.015],
    [0.05, 0.04],
    [0.047, 0.07],
    [0.034, 0.091],
    [0.025, 0.1],
    [0.025, 0.12],
    [0, 0.12],
  ],
  band: { h: 0.11, radius: 0.025, tube: 0.006 },
  bulb: {
    a: 0.01,
    h0: 0.12,
    profile: [
      [0, 0.12],
      [0.032, 0.124],
      [0.045, 0.148],
      [0.04, 0.178],
      [0.02, 0.197],
      [0, 0.2],
    ],
  },
  stalk: { h0: 0.195, r: 0.01 },
  knobs: [
    [0.009, 0.003, 0.2, 0.207, 0.0045],
    [0.005, -0.005, 0.212, 0.218, 0.004],
    [-0.012, 0.02, 0.183, 0.192, 0.005],
  ],
  tendrils: [
    [-0.03, 0.162, -0.055, 0.12, -0.012],
    [-0.03, 0.15, -0.05, 0.118, 0.012],
  ],
  tendril: { juice: 0.28, width: 0.006, hd: 0.003 },
  handle: { a: [0.04, 0.058], h: 0.05, r: 0.007 },
} as const;

/**
 * One pink gadget at `(a, d)`, scaled by `scale` in every dimension and,
 * with `flip`, turned half round (its tendrils to `+a`), up to `top` at
 * scale 1: the flat oval base (a box with two end cylinders), the lower
 * mass and pinched waist (one `lathe`), the mauve band round the waist,
 * the top bulb set off the axis, the thin knobby stalk up to the top, the
 * two tendrils hanging off one side with juice-tinted tips (each an
 * `extrude`d `tiltedBar`, rooted inside the bulb), and the small grey
 * handle out of the lower mass's other side.
 */
function gadget(
  kitAt: KitAt,
  s: Surfaces,
  a: number,
  d: number,
  scale: number,
  flip: boolean,
  top: number,
): void {
  const G = GADGET;
  const k = kitAt(yawed(ORIGIN, a, d, flip ? Math.PI : 0));
  const z = (x: number) => x * scale;
  const pink = s.tinted(GADGET_PINK);
  const fold = s.tinted(GADGET_FOLD);
  const B = G.base;
  k.box(z(-B.half), z(B.half), z(-B.hd), z(B.hd), 0, z(B.h), fold);
  for (const end of [-1, 1])
    k.cylinder(z(end * B.half), 0, 0, z(B.h), z(B.hd), 6, fold);
  k.lathe(
    0,
    0,
    G.lower.map(([r, h]) => [z(r), z(h)] as const),
    8,
    pink,
  );
  k.ring(0, 0, z(G.band.h), z(G.band.radius), z(G.band.tube), 3, 8, fold, "up");
  const U = G.bulb;
  k.lathe(
    z(U.a),
    0,
    U.profile.map(([r, h]) => [z(r), z(h)] as const),
    8,
    pink,
  );
  k.lathe(
    z(U.a),
    0,
    [
      [0, z(G.stalk.h0)],
      [z(G.stalk.r), z(G.stalk.h0)],
      [0, z(top)],
    ],
    6,
    pink,
  );
  for (const [ka, kd, h0, h1, r] of G.knobs)
    k.cylinder(z(U.a + ka), z(kd), z(h0), z(h1), z(r), 5, fold);
  const T = G.tendril;
  for (const [ra, rh, ta, th, td] of G.tendrils) {
    const ang = Math.atan2(th - rh, ta - ra);
    const cut = 1 - T.juice;
    const [ma, mh] = [ra + (ta - ra) * cut, rh + (th - rh) * cut];
    const bar = (x0: number, y0: number, x1: number, y1: number) =>
      tiltedBar(
        z((x0 + x1) / 2),
        z((y0 + y1) / 2),
        ang,
        z(Math.hypot(x1 - x0, y1 - y0)),
        z(T.width),
      );
    k.extrude(bar(ra, rh, ma, mh), z(td - T.hd), z(td + T.hd), pink);
    k.extrude(
      bar(ma, mh, ta, th),
      z(td - T.hd),
      z(td + T.hd),
      s.tinted(GADGET_JUICE),
    );
  }
  const Hd = G.handle;
  k.cylinderAlong(
    z(Hd.a[0]),
    z(Hd.a[1]),
    0,
    z(Hd.h),
    z(Hd.r),
    6,
    s.tinted(GADGET_HANDLE),
  );
}

/**
 * The cluster's three gadgets: `[a, d, scale, flip]`, a loose row whose
 * first, at scale 1, reaches the curio's top.
 */
const CLUSTER = [
  [-0.12, 0.02, 1, false],
  [0, -0.02, 0.85, true],
  [0.12, 0.02, 0.95, false],
] as const;

/**
 * The pink gadget: variant 0 one gadget on the origin, variant 1 three
 * in a loose row (`CLUSTER`), all through `gadget`. Nothing lights.
 */
const pinkGadget: CurioRecipe = ({ kitAt, s, variant, kind }) => {
  const { top } = curioHalf(kind, variant);
  if (variant === 0) {
    gadget(kitAt, s, 0, 0, 1, false, top);
    return;
  }
  for (const [a, d, scale, flip] of CLUSTER)
    gadget(kitAt, s, a, d, scale, flip, top);
};

/**
 * The meter's measures, in metres: the body (half width, `d`, `h`, bevel),
 * the grip (half width, half depth, `h`), the foot plate (half width, half
 * depth, height); on the front, the faceplate (half width, `d`, `h`), the
 * display's bezel and readout (half widths, their `d` and `h`) and the two
 * knobs (`a` either side, height, radius, how far out); the wings (their
 * hinge's `a` either side at the body's side, the knuckle's radius and
 * half length along `d`, the fan angle up from `a`, length, width and half
 * thickness) and the lights (side, how far they stand proud of the wing's
 * front face, and their distances from the hinge, root to tip, on the
 * left wing for positions 1 to 4 and on the right for 5 to 7); and
 * `chase`, the order the lights' positions light in: position `p` takes
 * blink group `chase.indexOf(p)`, so the chase bank visits them 3, 5, 7,
 * 4, 1, 6, 2 and then holds its dark beat on group 7.
 *
 * The hinge's height is not in the table: it is set so the wings' upper
 * edge ends exactly at the curio's top (`wingHinge`).
 */
export const METER = {
  body: { hw: 0.04, d: [-0.035, 0.035], h: [0.11, 0.24], bevel: 0.006 },
  grip: { hw: 0.02, hd: 0.022, h: [0.012, 0.11] },
  foot: { hw: 0.035, hd: 0.03, h: 0.012 },
  face: { hw: 0.032, d: [0.035, 0.039], h: [0.128, 0.228] },
  bezel: { hw: 0.022, d: [0.039, 0.041], h: [0.172, 0.212] },
  readout: { hw: 0.017, d: [0.041, 0.0425], h: [0.177, 0.207] },
  knob: { a: 0.013, h: 0.148, r: 0.005, out: 0.007 },
  wing: {
    a: 0.04,
    knuckle: { r: 0.008, hd: 0.012 },
    angle: (40 * Math.PI) / 180,
    length: 0.13,
    width: 0.018,
    hd: 0.006,
  },
  light: {
    side: 0.012,
    proud: 0.005,
    left: [0.032, 0.058, 0.084, 0.11],
    right: [0.04, 0.075, 0.11],
  },
  chase: [3, 5, 7, 4, 1, 6, 2] as readonly number[],
} as const;

/**
 * The height of the wings' hinges for a meter `top` tall: so low under
 * the body's upper corners that a wing fanned out and up at
 * `METER.wing.angle` ends with its upper edge exactly at `top`.
 */
function wingHinge(top: number): number {
  const W = METER.wing;
  return top - W.length * Math.sin(W.angle) - (W.width / 2) * Math.cos(W.angle);
}

/**
 * The winged meter: the black body on its short grip and flared foot
 * plate, the dark faceplate on its front with the display (a black bezel
 * and the green readout, a steady `s.signal`) and two tiny knobs, and the
 * two silver wings, each on a hinge knuckle at the body's side near its
 * upper corner, fanned out and up. The seven amber lights stand on the
 * wings' front faces, rotated with them, four on the left (`-a`) and
 * three on the right, and take their blink groups from `METER.chase`.
 */
const wingMeter: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { top } = curioHalf(kind, variant);
  const M = METER;
  const black = s.tinted(METER_BLACK);
  const dark = s.tinted(METER_GRIP);
  const silver = s.tinted(WING_SILVER, LAYER.metal);
  k.box(-M.foot.hw, M.foot.hw, -M.foot.hd, M.foot.hd, 0, M.foot.h, dark);
  k.box(
    -M.grip.hw,
    M.grip.hw,
    -M.grip.hd,
    M.grip.hd,
    M.grip.h[0],
    M.grip.h[1],
    dark,
  );
  const [bd0, bd1] = M.body.d;
  k.bevelBox(
    -M.body.hw,
    M.body.hw,
    bd0,
    bd1,
    M.body.h[0],
    M.body.h[1],
    M.body.bevel,
    black,
  );
  k.box(
    -M.face.hw,
    M.face.hw,
    M.face.d[0] - 0.001,
    M.face.d[1],
    M.face.h[0],
    M.face.h[1],
    dark,
  );
  k.box(
    -M.bezel.hw,
    M.bezel.hw,
    M.bezel.d[0],
    M.bezel.d[1],
    M.bezel.h[0],
    M.bezel.h[1],
    black,
  );
  const R = M.readout;
  k.box(-R.hw, R.hw, R.d[0], R.d[1], R.h[0], R.h[1], s.signal(READOUT_GREEN));
  const side = kitAt(yawed(ORIGIN, 0, 0, QUARTER));
  for (const a of [-M.knob.a, M.knob.a])
    side.cylinderAlong(
      M.face.d[1] - 0.0005,
      M.face.d[1] + M.knob.out,
      -a,
      M.knob.h,
      M.knob.r,
      6,
      black,
    );
  const W = M.wing;
  const hinge = wingHinge(top);
  const L = M.light;
  for (const dir of [-1, 1]) {
    const ha = dir * W.a;
    side.cylinderAlong(
      -W.knuckle.hd,
      W.knuckle.hd,
      -ha,
      hinge,
      W.knuckle.r,
      8,
      silver,
    );
    const ang = dir < 0 ? Math.PI - W.angle : W.angle;
    const along = (t: number): [number, number] => [
      ha + t * Math.cos(ang),
      hinge + t * Math.sin(ang),
    ];
    const [ca, ch] = along(W.length / 2);
    k.extrude(tiltedBar(ca, ch, ang, W.length, W.width), -W.hd, W.hd, silver);
    const spots = dir < 0 ? L.left : L.right;
    spots.forEach((t, i) => {
      const position = dir < 0 ? i + 1 : L.left.length + i + 1;
      const [la, lh] = along(t);
      k.extrude(
        tiltedBar(la, lh, ang, L.side, L.side),
        W.hd,
        W.hd + L.proud,
        s.blink(WING_AMBER, M.chase.indexOf(position)),
      );
    });
  }
};

/** The gear kinds' recipes, one per kind. */
export const GEAR_RECIPES = {
  "light-sword": lightSword,
  "green-pistol": greenPistol,
  "pink-gadget": pinkGadget,
  "wing-meter": wingMeter,
} satisfies Record<
  Extract<
    CurioKind,
    "light-sword" | "green-pistol" | "pink-gadget" | "wing-meter"
  >,
  CurioRecipe
>;
