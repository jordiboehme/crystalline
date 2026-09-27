/**
 * The street pieces: the red bike and the blue police box, two machines
 * from the street that ended up aboard. Both are lit on the `breathe`
 * bank (C13) and nothing on them moves.
 *
 * - The bike (`BIKE`) is a long racing motorcycle parked on its side
 *   stand, its length along `a` and its nose at `+a`: bright glossy red
 *   bodywork enclosing almost everything in one rounded capsule, narrow
 *   in plan, highest over its front third. A high rounded cowl slopes
 *   down and forward to the wide headlight, with a dark windscreen high
 *   on top of it leaning back and the handlebar's grips standing out
 *   each side just behind it; the saddle is sunk low behind the cowl,
 *   with a small backrest, and a rounded tail hump, lower than the cowl,
 *   rises over the rear wheel and ends in the round tail light, which
 *   breathes. A long wheelbase, fat black tyres with solid grey disc
 *   hubs, the front wheel showing under the nose and the rear one almost
 *   covered, and black under the chassis. It stands
 *   upright (C14): the kit has no roll, so the stand is a bar from the
 *   chassis down to the floor on its `-d` side. Four sponsor stickers
 *   (`BIKE_STICKERS`, 2.6f C13) sit on both sides of the shell: one large
 *   on the cowl's side under the windscreen, one small on the upper panel
 *   behind it, and two on the lower panels, over the chassis and over the
 *   rear wheel, each on a plate of its own or printed straight on the red.
 * - The police box is backed against a wall, its doors facing `+d`
 *   (C3, C10): deep blue painted wood on a low plinth, a corner post from
 *   the plinth to the roof at each corner, a black sign band under the
 *   roof on all four sides with its words in white block pixels
 *   (`BOX_SIGN`, laid out by `boxSignLayout`), a roof in two steps under
 *   a small lamp in a white cage that breathes, and on every side two
 *   columns of raised panels under a window of small frosted panes that
 *   glow faintly. On the front the two columns are the two door leaves,
 *   each built by `boxDoor` as a slab of its own, so a later stage can
 *   swing them inward; the left one carries the white door notice with its
 *   lines in black (`MARKS.boxNotice`, 2.6f C13).
 *   The body stands `BOX_BACK` off the wall, so the back sign stays in
 *   front of the wall plane.
 *
 * The numbers each kind is built to are named above its recipe: `BIKE`,
 * `BIKE_BODY` and `BIKE_PROFILE` for the bike, `BOX` for the box. Round parts use few
 * facets, as in every batch.
 */

import type { HeroKind } from "../../../world/types";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import {
  discOutline,
  offset,
  shade,
  sideways,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import { heroHalf, type HeroRecipe } from "./common";
import { MARKS } from "../marks";
import { fit, markLines, pixelPanel, textBlock, textRows } from "./pixels";

/** The recipe's own frame: the origin, facing north. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/** The bike's glossy red. */
const BIKE_RED: Rgb = [0.9, 0.08, 0.06];

/** The gloss on the top of the cowl: a lighter red where the light catches it. */
const BIKE_SHINE: Rgb = [0.98, 0.22, 0.16];

/** The bike's tyres: a near black rubber. */
const TYRE: Rgb = [0.06, 0.06, 0.07];

/** The bike's disc hubs: a mid grey. */
const HUB_GREY: Rgb = [0.45, 0.45, 0.47];

/** The bike's chassis, handlebar and stand: black. */
const BIKE_BLACK: Rgb = [0.05, 0.05, 0.05];

/** The bike's saddle: a reddish brown. */
const SADDLE: Rgb = [0.3, 0.14, 0.1];

/** The bike's tinted windscreen: nearly black with a cold cast. */
const SCREEN: Rgb = [0.05, 0.05, 0.08];

/** The bike's headlight: a steady warm white. */
const HEADLIGHT: Rgb = [1.0, 0.97, 0.85];

/** The bike's tail light: a red that breathes. */
const TAIL_LIGHT: Rgb = [0.95, 0.1, 0.08];

/**
 * The bike's overall size, in metres: `length` nose to tail along `a`,
 * `width` across its widest shell parts along `d`, and `screenTop`, the
 * top of its windscreen and so of the bike. These are the original's
 * sourced measures; the bike's headlight and tail light, standing a few
 * millimetres proud of its nose and tail, give the full length.
 */
export const BIKE = { length: 2.95, width: 0.83, screenTop: 1.17 } as const;

/**
 * The bike's shell and wheels, in metres:
 * - `end`: how far the shell reaches from the middle along `a`, a few
 *   millimetres short of half the length, so each light stands proud of
 *   the end it is set in rather than sharing its plane;
 * - `half`: the half width of the shell's upper panels over the seat and
 *   the cowl, as narrow as a motorbike's; `tailHalf` the tail hump's and
 *   `lower` the lower panels', hugging the wheels;
 * - `axle`: the wheels' centres along `a` (at `±axle`), `r` their radius
 *   (the axles stand at `r`, the tyres on the floor), `tyre` a tyre's
 *   half width, `hub` a disc hub's radius and `hubT` its thickness on
 *   the tyre's side wall;
 * - `lamp`: the headlight's height and radius (its height; it is
 *   `LAMP_WIDE` times as wide), `tail` the round tail light's.
 */
const BIKE_BODY = {
  end: 1.47,
  half: 0.26,
  tailHalf: 0.22,
  lower: 0.2,
  axle: 1.0,
  r: 0.33,
  tyre: 0.15,
  hub: 0.2,
  hubT: 0.02,
  lamp: [0.55, 0.07],
  tail: [0.62, 0.08],
} as const;

/** How many times wider than tall the headlight is. */
const LAMP_WIDE = 1.6;

/**
 * The shell's side profile, `[a, h]`, a teardrop capsule from the tail
 * round to the nose and back along the underside: the tail's flat end,
 * where the round tail light sits, the rounded tail hump over the rear
 * wheel, the backrest's drop into the seat sunk low behind the cowl, the
 * rise to the high rounded cowl whose top (the bike's highest shell) is
 * over the front third, and its slope down and forward to the nose's
 * flat front, where the headlight sits. Underneath, the shell is cut away
 * high over the front wheel (0.5), so the wheel shows under the nose; it
 * comes down to 0.3 over the chassis, and to 0.22 round the rear wheel,
 * which it almost covers.
 */
const BIKE_PROFILE: readonly (readonly [number, number])[] = [
  [-BIKE_BODY.end, 0.44],
  [-BIKE_BODY.end, 0.74],
  [-1.43, 0.81],
  [-1.35, 0.87],
  [-1.24, 0.9],
  [-1.12, 0.9],
  [-1.02, 0.87],
  [-0.94, 0.8],
  [-0.88, 0.66],
  [-0.2, 0.64],
  [-0.08, 0.7],
  [0.05, 0.8],
  [0.2, 0.9],
  [0.38, 0.98],
  [0.58, 1.03],
  [0.78, 1.04],
  [0.95, 1.0],
  [1.1, 0.93],
  [1.24, 0.84],
  [1.36, 0.75],
  [BIKE_BODY.end, 0.64],
  [BIKE_BODY.end, 0.46],
  [1.4, 0.44],
  [1.3, 0.5],
  [0.72, 0.5],
  [0.62, 0.3],
  [-0.62, 0.3],
  [-0.7, 0.22],
  [-1.3, 0.22],
  [-1.42, 0.32],
];

/**
 * The height the shell's lower panels end at: under it the shell is a
 * shade darker and narrower (`BIKE_BODY.lower`), close round the wheels,
 * so the bodywork reads as panels rather than one moulding.
 */
const SEAM = 0.56;

/**
 * The height over which the cowl's top wears `BIKE_SHINE`: the gloss the
 * light catches on the highest curve.
 */
const SHINE = 0.95;

/**
 * Where the tail hump meets the seat along `a`: behind it the shell
 * narrows to `BIKE_BODY.tailHalf`.
 */
const TAIL_CUT = -0.9;

/**
 * The part of a plane outline on one side of a line: of the points whose
 * coordinate `axis` (0 for `a`, 1 for `h`) is at most `v` with `below`,
 * at least `v` without (one pass of polygon clipping against a line). The
 * bike's shell is cut with it into panels of different widths and shades.
 */
function clipAt(
  poly: readonly (readonly [number, number])[],
  axis: 0 | 1,
  v: number,
  below: boolean,
): [number, number][] {
  const inside = (p: readonly [number, number]) =>
    below ? p[axis] <= v : p[axis] >= v;
  const out: [number, number][] = [];
  poly.forEach((p, i) => {
    const q = poly[(i + 1) % poly.length] ?? p;
    if (inside(p)) out.push([p[0], p[1]]);
    if (inside(p) !== inside(q)) {
      const t = (v - p[axis]) / (q[axis] - p[axis]);
      out.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
    }
  });
  return out;
}

/**
 * The windscreen's side profile, `[a, h]`: a dark shell set high on the
 * cowl, its top edge leaning back towards the rider up to
 * `BIKE.screenTop` over the front third and its lower edge running down
 * the cowl's forward slope a little under the cowl's top. It is a hair
 * wider than the cowl (`SCREEN_HALF`), so it wraps over the cowl's sides
 * as a dark band and shows from the side without sharing their planes.
 */
const SCREEN_PROFILE: readonly (readonly [number, number])[] = [
  [1.2, 0.86],
  [0.68, BIKE.screenTop],
  [0.6, BIKE.screenTop],
  [0.56, 1.0],
  [0.9, 0.94],
];

/** The windscreen's half width: 5 mm wider than the cowl on each side. */
const SCREEN_HALF = 0.265;

/**
 * The side stand's side profile, `[a', h]` in the `sideways` frame, where
 * `a'` is `-d`: a bar from inside the chassis's `-d` side down and out to
 * a flat foot on the floor.
 */
const STAND_PROFILE: readonly (readonly [number, number])[] = [
  [0.15, 0.27],
  [0.175, 0.27],
  [0.435, 0],
  [0.41, 0],
];

/** The tyres' tread: a darker rubber than their side walls. */
const TREAD: Rgb = [0.025, 0.025, 0.03];

/** The front brake disc's bright steel. */
export const BRAKE_STEEL: Rgb = [0.72, 0.72, 0.75];

/** The bezels round the lamps, the caliper and the instrument panel: a dark grey. */
const BIKE_DARK: Rgb = [0.14, 0.14, 0.16];

/** The accent line along each side: an off white. */
const ACCENT: Rgb = [0.88, 0.88, 0.84];

/** The indicator lenses beside the headlight: amber, off while parked. */
const INDICATOR: Rgb = [0.95, 0.55, 0.1];

/** The saddle's rolled edge: a darker brown than its top. */
const SADDLE_EDGE: Rgb = [0.16, 0.08, 0.06];

/**
 * The bike's small parts, in metres:
 * - `side`: a tyre's side wall starts this far in from the tread's half
 *   width and its radius is `wallIn` smaller, so the tread shows round
 *   the tyre as a darker band;
 * - `holes`: the radius the hubs' ring of six holes sits at, `hole` half
 *   a hole's side;
 * - `disc`: the brake disc's radius, `discT` its thickness, standing
 *   outboard of the front hub on the `+d` side;
 * - `fork`: the fork legs' width; each leg runs from the front axle up
 *   and back into the shell at `forkTop`;
 * - `bezel`: how much wider a lamp's bezel is than its lamp;
 * - `accent`: the accent line's heights; `seams` where the raised panel
 *   seams stand along `a`.
 */
const BIKE_PARTS = {
  side: 0.035,
  wallIn: 0.01,
  holes: 0.16,
  hole: 0.018,
  disc: 0.12,
  discT: 0.008,
  fork: 0.045,
  forkTop: [0.86, 0.58],
  bezel: 0.015,
  accent: [0.6, 0.62],
  seams: [-1.18, 0.3, 1.3],
} as const;

/**
 * The shell's top at `a`: the highest crossing of the vertical line at
 * `a` with `BIKE_PROFILE`, where a raised seam across the side ends.
 */
function shellTopAt(a: number): number {
  let top = -Infinity;
  BIKE_PROFILE.forEach((p, i) => {
    const q = BIKE_PROFILE[(i + 1) % BIKE_PROFILE.length] ?? p;
    if ((p[0] - a) * (q[0] - a) > 0 || p[0] === q[0]) return;
    top = Math.max(top, p[1] + ((a - p[0]) / (q[0] - p[0])) * (q[1] - p[1]));
  });
  return top;
}

/**
 * One sticker on the bike's fairing: which `MARKS.bikeStickers` entry, its
 * plate and ink, and where along the shell it sits: `a` along the bike and
 * `h` in height, on the `+d` side, mirrored on the `-d` side.
 */
export interface BikeSticker {
  text: number;
  /** The plate's colour; `null`: the letters print straight on the red. */
  plate: Rgb | null;
  ink: Rgb;
  a: readonly [number, number];
  h: readonly [number, number];
}

/**
 * The bike's four sponsor stickers (2.6f C13), placed as the sticker sets
 * copied from the original place them, on the flat sides of the shell:
 * - the camera maker's, the largest, red letters on a white plate on the
 *   cowl's side, between the seam at 0.3 and the nose, under the
 *   windscreen's lower edge and over the accent line;
 * - a helmet maker's in white letters straight on the red of the upper
 *   panel between the seat and the seam at 0.3, under the shell's top as
 *   it rises to the cowl;
 * - the watch maker's in black on a white plate on the lower panel over
 *   the chassis, between the footpeg and the front wheel;
 * - the other helmet maker's in white on a black plate on the lower panel
 *   over the rear wheel, behind the vents.
 * Every ink is its sticker's own, used by no other part of the bike, and
 * none is the bike's red, shine or black.
 */
export const BIKE_STICKERS: readonly BikeSticker[] = [
  {
    text: 0,
    plate: [0.96, 0.96, 0.93],
    ink: [0.8, 0.05, 0.05],
    a: [0.36, 1.02],
    h: [0.66, 0.84],
  },
  {
    text: 1,
    plate: [0.94, 0.94, 0.91],
    ink: [0.02, 0.02, 0.03],
    a: [-0.2, 0.56],
    h: [0.34, 0.54],
  },
  {
    text: 2,
    plate: null,
    ink: [1.0, 1.0, 1.0],
    a: [0.0, 0.28],
    h: [0.64, 0.74],
  },
  {
    text: 3,
    plate: [0.03, 0.03, 0.035],
    ink: [0.97, 0.97, 0.97],
    a: [-1.28, -0.72],
    h: [0.3, 0.52],
  },
];

/**
 * A mark's box inset by one pixel of the block fitted into it, so the
 * letters stand clear of its plate's (or notice's) edge.
 */
function insetBox(
  lines: string | readonly string[],
  [a0, a1, h0, h1]: readonly [number, number, number, number],
): [number, number, number, number] {
  const rows = textBlock(typeof lines === "string" ? [lines] : lines);
  const m = fit(rows, a0, a1, h0, h1).px;
  return [a0 + m, a1 - m, h0 + m, h1 - m];
}

/**
 * The red bike: the shell over two fat wheels with a dark tread band and
 * grey disc hubs with a ring of holes, the front one with a steel brake
 * disc, its caliper and the fork legs; the chassis, footpegs and the
 * side stand under it; the saddle with its rolled edge and the backrest;
 * the wrap-round windscreen with the instrument panel behind it and the
 * handlebar whose grips, each with a mirror, stand out each side of the
 * cowl; the headlight in its bezel between two indicators; the tail light
 * in its bezel over a small bracket; and on each side the raised panel
 * seams, the accent line and the vents ahead of the rear wheel.
 */
const redBike: HeroRecipe = ({ k, kitAt, s }) => {
  const b = BIKE_BODY;
  const P = BIKE_PARTS;
  const hub = s.tinted(HUB_GREY);
  const black = s.tinted(BIKE_BLACK);
  const dark = s.tinted(BIKE_DARK);
  // The back view, for the decals on the `-d` sides: a' is -a, d' is -d.
  const back = kitAt(yawed(ORIGIN, 0, 0, Math.PI));
  const wall = b.tyre - P.side;
  for (const a of [-b.axle, b.axle]) {
    k.extrude(
      discOutline(a, b.r, b.r, 14, "top"),
      -wall,
      wall,
      s.tinted(TREAD),
    );
    const walls = discOutline(a, b.r, b.r - P.wallIn, 14, "top");
    k.extrude(walls, wall - 0.005, b.tyre, s.tinted(TYRE));
    k.extrude(walls, -b.tyre, -wall + 0.005, s.tinted(TYRE));
    const disc = discOutline(a, b.r, b.hub, 12, "top");
    k.extrude(disc, b.tyre, b.tyre + b.hubT, hub);
    k.extrude(disc, -b.tyre - b.hubT, -b.tyre, hub);
    const face = b.tyre + b.hubT + DECAL_LIFT;
    for (let i = 0; i < 6; i++) {
      const t = ((2 * i + 1) * Math.PI) / 6;
      const ha = a + P.holes * Math.cos(t);
      const hh = b.r + P.holes * Math.sin(t);
      const [h0, h1] = [hh - P.hole, hh + P.hole];
      k.panel(ha - P.hole, ha + P.hole, face, h0, h1, black);
      back.panel(-ha - P.hole, -ha + P.hole, face, h0, h1, black);
    }
  }
  // The front brake disc outboard of the hub, its caliper and the fork.
  const d0 = b.tyre + b.hubT + 0.005;
  k.extrude(
    discOutline(b.axle, b.r, P.disc, 12, "top"),
    d0,
    d0 + P.discT,
    s.tinted(BRAKE_STEEL),
  );
  k.box(0.88, 0.95, d0 - 0.01, d0 + 0.02, 0.3, 0.36, dark);
  const [fa, fh] = P.forkTop;
  for (const d of [-1, 1])
    k.extrude(
      [
        [b.axle + P.fork / 2, b.r],
        [fa + P.fork / 2, fh],
        [fa - P.fork / 2, fh],
        [b.axle - P.fork / 2, b.r],
      ],
      d * 0.19,
      d * 0.215,
      black,
    );

  const upper = clipAt(BIKE_PROFILE, 1, SEAM, false);
  const red = s.tinted(BIKE_RED);
  const front = clipAt(upper, 0, TAIL_CUT, false);
  k.extrude(clipAt(front, 1, SHINE, true), -b.half, b.half, red);
  k.extrude(
    clipAt(front, 1, SHINE, false),
    -b.half,
    b.half,
    s.tinted(BIKE_SHINE),
  );
  k.extrude(clipAt(upper, 0, TAIL_CUT, true), -b.tailHalf, b.tailHalf, red);
  k.extrude(
    clipAt(BIKE_PROFILE, 1, SEAM, true),
    -b.lower,
    b.lower,
    s.tinted(shade(BIKE_RED, 0.75)),
  );
  k.box(-0.6, 0.6, -0.16, 0.16, 0.22, 0.3, black);

  // Each side: the accent line, the raised seams, the vents and a footpeg.
  const accent = s.tinted(ACCENT);
  const seam = s.tinted(shade(BIKE_RED, 0.85));
  const [ah0, ah1] = P.accent;
  for (const d of [-1, 1]) {
    k.box(
      TAIL_CUT + 0.02,
      1.44,
      d * (b.half - 0.005),
      d * (b.half + 0.004),
      ah0,
      ah1,
      accent,
    );
    k.box(
      -1.45,
      TAIL_CUT - 0.02,
      d * (b.tailHalf - 0.005),
      d * (b.tailHalf + 0.004),
      ah0,
      ah1,
      accent,
    );
    for (const sa of P.seams) {
      const half = sa < TAIL_CUT ? b.tailHalf : b.half;
      k.box(
        sa - 0.006,
        sa + 0.006,
        d * (half - 0.005),
        d * (half + 0.008),
        SEAM + 0.01,
        shellTopAt(sa) - 0.015,
        seam,
      );
    }
    k.box(-0.33, -0.27, d * 0.19, d * 0.28, 0.33, 0.36, black);
  }
  for (let i = 0; i < 4; i++) {
    const h = 0.35 + i * 0.04;
    k.panel(-0.6, -0.36, b.lower + DECAL_LIFT, h, h + 0.02, black);
    back.panel(0.36, 0.6, b.lower + DECAL_LIFT, h, h + 0.02, black);
  }

  const saddle = s.tinted(SADDLE);
  k.bevelBox(
    -0.86,
    -0.24,
    -0.19,
    0.19,
    0.635,
    0.675,
    0.015,
    s.tinted(SADDLE_EDGE),
  );
  k.bevelBox(-0.85, -0.25, -0.18, 0.18, 0.64, 0.7, 0.02, saddle);
  // The small backrest, against the tail hump behind the saddle.
  k.bevelBox(-0.94, -0.87, -0.14, 0.14, 0.66, 0.85, 0.02, saddle);

  k.extrude(SCREEN_PROFILE, -SCREEN_HALF, SCREEN_HALF, s.tinted(SCREEN));
  // The instrument panel just behind the screen, then the handlebar with
  // its grips out each side of the cowl and a mirror on each grip.
  k.box(0.51, 0.555, -0.13, 0.13, 1.0, 1.1, dark);
  k.box(0.45, 0.5, -0.34, 0.34, 1.0, 1.03, black);
  for (const d of [-1, 1]) {
    k.box(0.44, 0.51, d * 0.28, d * 0.37, 0.99, 1.04, black);
    k.box(0.47, 0.48, d * 0.33, d * 0.34, 1.04, 1.1, black);
    k.box(0.465, 0.485, d * 0.31, d * 0.39, 1.1, 1.15, dark);
  }

  const side = kitAt(sideways(ORIGIN));
  const nose = BIKE.length / 2;
  const [lampH, lampR] = b.lamp;
  const oval = (r: number) =>
    discOutline(0, lampH, r, 10).map(([x, h]): [number, number] => [
      x * LAMP_WIDE,
      h,
    ]);
  side.extrude(oval(lampR), nose - 0.01, nose, s.signal(HEADLIGHT));
  side.extrude(oval(lampR + P.bezel), nose - 0.01, nose - 0.004, dark);
  for (const d of [-1, 1])
    side.extrude(
      discOutline(-d * 0.19, 0.6, 0.025, 8),
      nose - 0.01,
      nose - 0.002,
      s.tinted(INDICATOR),
    );
  const [tailH, tailR] = b.tail;
  side.extrude(
    discOutline(0, tailH, tailR, 10),
    -nose,
    -nose + 0.01,
    s.blink(TAIL_LIGHT, 0),
  );
  side.extrude(
    discOutline(0, tailH, tailR + P.bezel, 10),
    -nose + 0.004,
    -nose + 0.01,
    dark,
  );
  k.box(-1.465, -1.455, -0.1, 0.1, 0.34, 0.45, black);
  side.extrude(STAND_PROFILE, -0.22, -0.18, black);

  // The stickers, on each side's flat panel: the upper panels at `half`,
  // the lower ones (under `SEAM`) at `lower`. The `-d` side mirrors `a`.
  // A plate stands one lift proud and its letters one more; letters
  // printed straight on the red stand one lift proud, as any decal does.
  for (const st of BIKE_STICKERS) {
    const text = MARKS.bikeStickers[st.text] ?? "";
    const [a0, a1] = st.a;
    const [h0, h1] = st.h;
    const face = h1 <= SEAM ? b.lower : b.half;
    const ink = s.tinted(st.ink);
    for (const [kk, s0, s1] of [
      [k, a0, a1],
      [back, -a1, -a0],
    ] as const) {
      if (st.plate !== null)
        kk.panel(s0, s1, face + DECAL_LIFT, h0, h1, s.tinted(st.plate));
      markLines(
        kk,
        text,
        insetBox(text, [s0, s1, h0, h1]),
        face + (st.plate === null ? 1 : 2) * DECAL_LIFT,
        ink,
      );
    }
  }
};

/** The box's deep blue paint. */
const BLUE: Rgb = [0.05, 0.15, 0.35];

/** The sign band's black. */
const SIGN_BLACK: Rgb = [0.05, 0.05, 0.05];

/** The white of the sign's words, the window frames, the notice and the lamp's cage. */
const WHITE: Rgb = [0.95, 0.95, 0.92];

/** The frosted panes' faint steady glow. */
const PANE: Rgb = [0.62, 0.62, 0.55];

/** The roof lamp's warm white, which breathes. */
const LAMP: Rgb = [1.0, 0.95, 0.8];

/** The door handles: a dark iron. */
const HANDLE: Rgb = [0.08, 0.08, 0.09];

/** The door notice's black ink. */
export const NOTICE_INK: Rgb = [0.05, 0.05, 0.06];

/**
 * How far the police box's body stands off its wall, in metres: the back
 * sign band and its words stand this side of the wall plane (C10).
 */
export const BOX_BACK = 0.06;

/**
 * The police box's measures, in metres, `a` across its front and `d` out
 * from the wall:
 * - `half`: the half width of the plinth, the posts' outer faces and the
 *   cornice; `front`: their front, the footprint's depth;
 * - `plinth`: the plinth's height;
 * - `post`: a corner post's side; the posts run from the plinth up to
 *   `eave`, under the cornice;
 * - `wall`: the body's half width, `post / 2` inside the posts, and
 *   `face`: the plane of the front doors, as far inside the posts' front;
 * - `leaf`: a door leaf's thickness; the body's front stands this far
 *   behind `face`, so the leaves are slabs of their own and share no
 *   plane with the body;
 * - `band0` and `band1`: the sign band's bottom and top, `bandT` how far
 *   it stands proud of its face;
 * - `cornice`, `tier1` and `tier2`: the tops of the roof's three steps,
 *   `inset1` and `inset2` how far the upper two are set in; they stay
 *   thin, so the lamp (`ROOF_LAMP`) stands clear of the cornice's edge
 *   seen from the floor;
 * - `signW`: the width every face's sign is laid out to, a little inside
 *   the narrowest clear span between two posts (the sides, 1.04 m).
 */
const BOX = {
  half: 0.65,
  front: 1.3,
  plinth: 0.1,
  post: 0.1,
  eave: 2.3,
  wall: 0.6,
  face: 1.25,
  leaf: 0.03,
  band0: 2.08,
  band1: 2.26,
  bandT: 0.03,
  cornice: 2.37,
  tier1: 2.43,
  tier2: 2.47,
  inset1: 0.07,
  inset2: 0.15,
  signW: 1.0,
} as const;

/** Half the width of the meeting stile between the two door leaves, in metres. */
const STILE = 0.02;

/**
 * The roof lamp's measures, in metres: a blue base from the roof's top
 * step up to `glass0`, `rim` in radius; the breathing lamp glass from
 * there to `glass1`, `glass` in radius; four white cage bars `bar` square
 * round it, their centres `bars` from its axis; a white cap from `glass1`
 * to `cap`, as wide as the base, and a small finial on it up to the top.
 */
const ROOF_LAMP = {
  glass0: 2.5,
  glass1: 2.64,
  cap: 2.67,
  rim: 0.075,
  glass: 0.055,
  bars: 0.068,
  bar: 0.01,
  finial: 0.02,
} as const;

/**
 * A panel column's measures, in metres over its face: `inset` from the
 * column's sides to its raised panels and its window; the three raised
 * panels' heights `panels`, standing `raised` proud of the face; the
 * window's frame from `win0` to `win1`, `frameT` proud of the face, with
 * `paneCols` by `paneRows` panes in it, `bar` apart (the white glazing
 * bars that show between them) and as far in from the frame's edge.
 */
const COLUMN = {
  inset: 0.05,
  panels: [
    [0.2, 0.65],
    [0.72, 1.15],
    [1.22, 1.6],
  ],
  raised: 0.015,
  win0: 1.66,
  win1: 2.02,
  frameT: 0.01,
  paneCols: 3,
  paneRows: 2,
  bar: 0.02,
} as const;

/**
 * The shades of the box's blue, as painted wood weathers unevenly: each
 * raised panel of a column a little lighter than the face, each in its
 * own shade, the posts and the roof's steps each their own.
 */
const PANEL_SHADES = [1.1, 1.02, 1.06] as const;

/** Half the width of the white notice on the left door leaf, in metres. */
const NOTICE_HALF = 0.14;

/**
 * One word of the sign as `boxSignLayout` places it: its pixel rows
 * (`textRows`), the left edge `a0`, the top `h1` and the pixel size `px`,
 * ready for `pixelPanel`.
 */
export interface SignWord {
  rows: string[];
  a0: number;
  h1: number;
  px: number;
}

/**
 * The words on the police box's sign band: one of the three approved
 * exceptions to the no-markings rule, with the core wall's nameplate and
 * the block's mark. These four words only, set in the station's block
 * pixels; every identifier and test round them stays generic.
 */
export const BOX_SIGN = {
  left: "POLICE",
  upper: "PUBLIC",
  lower: "CALL",
  right: "BOX",
} as const;

/**
 * The sign laid out across a band `width` wide, centred on `a` 0 and on
 * the height `mid`, as the original sets it (C12): the first and last
 * words large at either end, and the two middle words stacked, the upper
 * over the lower, in smaller pixels between them, left aligned in their
 * slot. The two small words and the gap between them (one small pixel
 * row) are exactly as tall as a large word, since a small pixel is 5/11 of
 * a large one; the large words are 23 and 11 columns wide, and each gap
 * between the three blocks is 3 large pixels. Returned in reading order:
 * the left word, the upper, the lower, the right.
 */
export function boxSignLayout(width: number, mid: number): SignWord[] {
  const big = width / (23 + 11 + 6 + (23 * 5) / 11);
  const small = (big * 5) / 11;
  const top = mid + 2.5 * big;
  const stack = -width / 2 + 26 * big;
  return [
    { rows: textRows(BOX_SIGN.left), a0: -width / 2, h1: top, px: big },
    { rows: textRows(BOX_SIGN.upper), a0: stack, h1: top, px: small },
    {
      rows: textRows(BOX_SIGN.lower),
      a0: stack,
      h1: top - 6 * small,
      px: small,
    },
    {
      rows: textRows(BOX_SIGN.right),
      a0: width / 2 - 11 * big,
      h1: top,
      px: big,
    },
  ];
}

/**
 * One column of the box's panelling on a face at depth `d` of `k`, from
 * `c0` to `c1` along it: three raised panels in their own shades of blue,
 * the white window frame over them and its frosted panes, which glow
 * faintly and leave the frame's white showing between them as glazing
 * bars. With `notice`, the white notice sits on the top raised panel,
 * its lines (`MARKS.boxNotice`) set on it in black. The door leaves and the other three sides share it.
 */
function facePanels(
  k: Kit,
  s: Surfaces,
  d: number,
  c0: number,
  c1: number,
  notice: boolean,
): void {
  const C = COLUMN;
  const p0 = c0 + C.inset;
  const p1 = c1 - C.inset;
  C.panels.forEach(([h0, h1], i) =>
    k.box(
      p0,
      p1,
      d,
      d + C.raised,
      h0,
      h1,
      s.tinted(shade(BLUE, PANEL_SHADES[i] ?? 1)),
    ),
  );
  k.box(p0, p1, d, d + C.frameT, C.win0, C.win1, s.tinted(WHITE));
  const pw = (p1 - p0 - (C.paneCols + 1) * C.bar) / C.paneCols;
  const ph = (C.win1 - C.win0 - (C.paneRows + 1) * C.bar) / C.paneRows;
  const glass = s.signal(PANE);
  for (let i = 0; i < C.paneCols; i++)
    for (let j = 0; j < C.paneRows; j++) {
      const a = p0 + C.bar + i * (pw + C.bar);
      const h = C.win0 + C.bar + j * (ph + C.bar);
      k.panel(a, a + pw, d + C.frameT + DECAL_LIFT, h, h + ph, glass);
    }
  if (!notice) return;
  const n = (p0 + p1) / 2;
  k.box(
    n - NOTICE_HALF,
    n + NOTICE_HALF,
    d + C.raised,
    d + C.raised + DECAL_LIFT,
    1.3,
    1.52,
    s.tinted(WHITE),
  );
  markLines(
    k,
    MARKS.boxNotice,
    insetBox(MARKS.boxNotice, [n - NOTICE_HALF, n + NOTICE_HALF, 1.3, 1.52]),
    d + C.raised + 2 * DECAL_LIFT,
    s.tinted(NOTICE_INK),
  );
}

/**
 * One door leaf of the police box, built into the kits `kitAt` makes at
 * the leaf's closed place. It is one call so that 2.6e can build it into
 * a mover and swing it inward; in 2.6c it stands closed (C10).
 *
 * The leaf is built in a frame at its hinge: the vertical line at its
 * outer edge (`±leafEdge`) on the leaf's inner face, so swinging it is a
 * turn of that frame about its origin. It is a slab `BOX.leaf` thick from
 * the body's front out to the door plane, with its column of panels
 * (`facePanels`) on the slab's front and a dark handle at its inner edge.
 * The right leaf (at `+a`, the viewer's right) carries the meeting stile
 * that covers the joint between the two; the left one the door notice.
 * Nothing else of the box is built here, so the call carries the whole
 * leaf and only it.
 */
export function boxDoor(
  kitAt: KitAt,
  s: Surfaces,
  leaf: "left" | "right",
): void {
  const edge = BOX.half - BOX.post + 0.01;
  const sign = leaf === "right" ? 1 : -1;
  const k = kitAt(offset(ORIGIN, sign * edge, BOX.face - BOX.leaf));
  // Along the leaf from its hinge (0) to its inner edge (`inner`).
  const inner = sign * -(edge - STILE);
  const [c0, c1] = inner < 0 ? [inner, 0] : [0, inner];
  const h1 = BOX.band0;
  k.box(c0, c1, 0, BOX.leaf, BOX.plinth, h1, s.tinted(shade(BLUE, 0.86)));
  facePanels(k, s, BOX.leaf, c0, c1, leaf === "left");
  const hand = inner + sign * 0.01;
  k.box(
    Math.min(hand, hand + sign * 0.025),
    Math.max(hand, hand + sign * 0.025),
    BOX.leaf,
    BOX.leaf + 0.025,
    1.07,
    1.13,
    s.tinted(HANDLE),
  );
  if (leaf === "right")
    k.box(
      -edge - STILE,
      -edge + STILE,
      0,
      BOX.leaf + 0.008,
      BOX.plinth,
      h1,
      s.tinted(shade(BLUE, 0.85)),
    );
}

/**
 * The blue police box: the plinth, the body, the four corner posts, the
 * sign band with its words on all four sides, the two door leaves on the
 * front (`boxDoor`) and the matching panels on the other three sides,
 * the roof's three steps and the lamp in its cage on top.
 */
const policeBox: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { top } = heroHalf(kind, variant);
  const B = BOX;
  const back = BOX_BACK;
  const blue = (f: number) => s.tinted(shade(BLUE, f));
  k.box(-B.half, B.half, back, B.front, 0, B.plinth, blue(0.8));
  // The body: its front behind the door leaves, its top under the cornice.
  const d0 = back + B.post / 2;
  k.box(-B.wall, B.wall, d0, B.face - B.leaf, B.plinth, B.eave, blue(0.84));
  const ph = B.half - B.post;
  for (const a of [-1, 1])
    for (const [p0, p1] of [
      [back, back + B.post],
      [B.front - B.post, B.front],
    ] as const)
      k.box(a * ph, a * B.half, p0, p1, B.plinth, B.eave, blue(1.0));

  // Every face seen from the body's middle: the front, the back and the
  // two sides, each with its half width to the posts and its depth.
  const mid = (d0 + B.face) / 2;
  const centre = offset(ORIGIN, 0, mid);
  const halfD = (B.face - d0) / 2;
  const faces: { f: Frame; depth: number; clear: number; front: boolean }[] = [
    { f: ORIGIN, depth: B.face, clear: ph, front: true },
    { f: yawed(centre, 0, 0, Math.PI), depth: halfD, clear: ph, front: false },
    {
      f: yawed(centre, 0, 0, Math.PI / 2),
      depth: B.wall,
      clear: B.front - B.post - mid,
      front: false,
    },
    {
      f: yawed(centre, 0, 0, -Math.PI / 2),
      depth: B.wall,
      clear: B.front - B.post - mid,
      front: false,
    },
  ];
  const band = s.tinted(SIGN_BLACK);
  const white = s.signal(WHITE);
  const words = boxSignLayout(B.signW, (B.band0 + B.band1) / 2);
  for (const { f, depth, clear, front } of faces) {
    const fk = kitAt(f);
    // The front's band closes the gap over the leaves down to the body.
    const b0 = front ? depth - B.leaf : depth;
    fk.box(-clear, clear, b0, depth + B.bandT, B.band0, B.band1, band);
    for (const w of words)
      pixelPanel(
        fk,
        w.rows,
        w.a0,
        w.h1,
        w.px,
        depth + B.bandT + DECAL_LIFT,
        (ch) => (ch === "#" ? white : null),
      );
    if (front) continue;
    facePanels(fk, s, depth, -clear, -STILE, false);
    facePanels(fk, s, depth, STILE, clear, false);
  }
  boxDoor(kitAt, s, "left");
  boxDoor(kitAt, s, "right");

  // The roof: the cornice over the posts, then two smaller steps.
  k.box(-B.half, B.half, back, B.front, B.eave, B.cornice, blue(1.0));
  const i1 = B.inset1;
  k.box(
    -B.half + i1,
    B.half - i1,
    back + i1,
    B.front - i1,
    B.cornice,
    B.tier1,
    blue(0.88),
  );
  const i2 = B.inset2;
  k.box(
    -B.half + i2,
    B.half - i2,
    back + i2,
    B.front - i2,
    B.tier1,
    B.tier2,
    blue(0.78),
  );

  // The lamp in its white cage.
  const L = ROOF_LAMP;
  const lampD = (back + B.front) / 2;
  const cage = s.tinted(WHITE);
  k.cylinder(0, lampD, B.tier2, L.glass0, L.rim, 10, blue(0.9));
  k.cylinder(0, lampD, L.glass0, L.glass1, L.glass, 10, s.blink(LAMP, 0));
  for (let i = 0; i < 4; i++) {
    const t = (i * Math.PI) / 2 + Math.PI / 4;
    const a = L.bars * Math.cos(t);
    const d = lampD + L.bars * Math.sin(t);
    const w = L.bar / 2;
    k.box(a - w, a + w, d - w, d + w, L.glass0, L.glass1, cage);
  }
  k.cylinder(0, lampD, L.glass1, L.cap, L.rim, 10, cage);
  k.cylinder(0, lampD, L.cap, top, L.finial, 6, cage);
};

/** The street kinds' recipes. */
export const STREET_RECIPES = {
  "red-bike": redBike,
  "police-box": policeBox,
} satisfies Record<Extract<HeroKind, "red-bike" | "police-box">, HeroRecipe>;
