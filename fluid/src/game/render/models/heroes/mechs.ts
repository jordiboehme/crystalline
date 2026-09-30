/**
 * The machines' recipes: the giant robot head, the spider tank and the
 * garden robot, the big machines of the engineering and lab halls. What
 * they share is that each has living eyes: its lenses breathe on the
 * `breathe` bank (C13), and nothing moves. Each faces `+d` (C3).
 *
 * - The head is the helmeted head of a giant violet bio-machine, set down
 *   as a spare part in a low dark-grey maintenance cradle: two rails and
 *   two cross beams on the floor, a saddle under its neck, uprights on
 *   the rails, a clamp from each side in to the neck and three black
 *   hoses from the rails into the neck. The helmet is deep violet with
 *   lime-green trim lines: a long, narrow skull swept back and up, built
 *   of a row of overlapping squashed balls; a dark face with two narrow,
 *   slanted yellow-green eyes deep under a hard chevron brow; a forehead
 *   ridge from which one horn grows just above the brow, curving forward
 *   and up in tapering segments to the hero's top, with a lime band; a
 *   long jaw guard jutting to a point, its upper edge stepped like teeth
 *   and a lime line along its lower edge, narrowing in three slices;
 *   cheek plates on both sides sweeping back and up, edged in lime; and a
 *   hooked crest at the skull's back. No number, no emblem.
 * - The tank is pale sky blue with white joints and darker grey
 *   underparts: a big rounded pod abdomen at its back (two overlapping
 *   ovoids), a white waist, a smaller cabin in front with three round
 *   lenses (`TANK_LENSES`: two side by side, the larger one below), a
 *   short gun pod on the cabin's roof, two small white manipulator arms
 *   under its front and four stout jointed legs, each rising from its hip
 *   to a high knee and down to a round wheel foot on the floor, the
 *   wheels turned to roll along `d`.
 * - The garden robot is weathered rust with moss: a round body, broad at
 *   its shoulders and narrowing to its hips, in bands of rust that differ
 *   a little in shade; two stubby block legs with flat feet; a row of dark
 *   plates round its middle; round shoulder joints; very long thin arms
 *   of flat segments on a dark core, bent a little at the elbow, with
 *   three-fingered hands hanging almost to the floor; a small domed head
 *   on a thin neck, with one amber eye in a dark socket on a lighter face
 *   plate; moss draped over its upper body and capping its shoulders; a
 *   small bird on its right shoulder (`+a`); and a few flowers growing
 *   from a crack in its front.
 *
 * The numbers each kind is built to are named in a table above its
 * recipe: `HEAD`, `TANK` with `TANK_LENSES` and `TANK_LEGS`, and `ROBOT`.
 * Round parts use few facets, as in every batch.
 */

import type { HeroKind } from "../../../world/types";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Surface } from "../../geometry";
import type { Rgb } from "../../looks";
import {
  discOutline,
  shade,
  sideways,
  tiltedBar,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import { heroHalf, type HeroRecipe } from "./common";

/** The recipe's own frame: the origin, facing north. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/** A point in a 2D plane: `(a, h)`, or `(d, h)` for a side profile. */
type P2 = readonly [number, number];

/**
 * A flat bar from `p` to `q` in a 2D plane, `width` wide: `tiltedBar`
 * given its two ends rather than its centre, length and angle.
 */
function barBetween(p: P2, q: P2, width: number): [number, number][] {
  const dx = q[0] - p[0];
  const dy = q[1] - p[1];
  return tiltedBar(
    (p[0] + q[0]) / 2,
    (p[1] + q[1]) / 2,
    Math.atan2(dy, dx),
    Math.hypot(dx, dy),
    width,
  );
}

/** The point `t` of the way from `p` to `q`. */
const lerp2 = (p: P2, q: P2, t: number): [number, number] => [
  p[0] + (q[0] - p[0]) * t,
  p[1] + (q[1] - p[1]) * t,
];

/**
 * A side profile of `[d, h]` points made ready for `extrude` in
 * `sideways(ORIGIN)`: there a point `(a', d')` is `(d', -a')` in the
 * recipe's frame, so `d` goes in as `-d`, and the extrusion's depth range
 * is the recipe's `a`.
 */
const across = (pts: readonly P2[]): [number, number][] =>
  pts.map(([d, h]) => [-d, h]);

/** The head's armour: a deep violet. */
const HEAD_VIOLET: Rgb = [0.35, 0.15, 0.45];

/** The head's trim lines: lime green. */
const HEAD_LIME: Rgb = [0.65, 0.85, 0.15];

/** The head's eyes: a yellow-green that breathes. */
const HEAD_EYE: Rgb = [0.8, 1.0, 0.3];

/** The dark face the eyes sit in, deep under the brow. */
const HEAD_FACE: Rgb = [0.08, 0.05, 0.1];

/** The cradle: a dark grey. */
const CRADLE_GREY: Rgb = [0.25, 0.26, 0.28];

/** The neck opening: near black. */
const NECK_DARK: Rgb = [0.1, 0.1, 0.11];

/** The hoses: black rubber. */
const HOSE_BLACK: Rgb = [0.05, 0.05, 0.06];

/**
 * A squashed ball round the vertical axis at `(a, d)`: radius `r`, its
 * middle at `h`, its lower half `down` and its upper half `up` high, with
 * `sides` facets and rings every 30 degrees. The skull is built of a row
 * of these.
 */
function ovoid(
  k: Kit,
  [a, d, h, r, down, up]: readonly [
    number,
    number,
    number,
    number,
    number,
    number,
  ],
  sides: number,
  s: Surface,
): void {
  const ring = (deg: number): [number, number] => {
    const t = (deg * Math.PI) / 180;
    const x = Math.abs(deg) === 90 ? 0 : r * Math.cos(t);
    return [x, h + (deg < 0 ? down : up) * Math.sin(t)];
  };
  k.lathe(a, d, [-90, -60, -30, 0, 30, 60, 90].map(ring), sides, s);
}

/**
 * A tapered bar through `pts` (`[d, h]`) in the side plane, `widths`
 * wide at each point (0 at a sharp tip). At an inner point its edge
 * follows the mean of the two segments' normals, so the pieces meet
 * edge to edge. One outline per segment, so a caller can give each its
 * own thickness.
 */
function taperedSegments(
  pts: readonly P2[],
  widths: readonly number[],
): [number, number][][] {
  const normal = (p: P2, q: P2): [number, number] => {
    const len = Math.hypot(q[0] - p[0], q[1] - p[1]);
    return [-(q[1] - p[1]) / len, (q[0] - p[0]) / len];
  };
  const at = pts.map((p, i) => {
    const prev = pts[i - 1];
    const next = pts[i + 1];
    const n0 = prev ? normal(prev, p) : null;
    const n1 = next ? normal(p, next) : null;
    const n = n0 && n1 ? [n0[0] + n1[0], n0[1] + n1[1]] : (n0 ?? n1 ?? [0, 1]);
    const len = Math.hypot(n[0] ?? 0, n[1] ?? 1);
    const w = (widths[i] ?? 0) / 2;
    const nx = ((n[0] ?? 0) / len) * w;
    const ny = ((n[1] ?? 1) / len) * w;
    return {
      lo: [p[0] - nx, p[1] - ny] as [number, number],
      hi: [p[0] + nx, p[1] + ny] as [number, number],
      tip: w === 0,
    };
  });
  const out: [number, number][][] = [];
  for (let i = 0; i + 1 < at.length; i++) {
    const p = at[i];
    const q = at[i + 1];
    if (!p || !q) continue;
    out.push(q.tip ? [p.lo, q.lo, p.hi] : [p.lo, q.lo, q.hi, p.hi]);
  }
  return out;
}

/**
 * The giant robot head's measures, in metres (its footprint and top are
 * the catalogue's, read through `heroHalf`). The skull is long, narrow
 * and swept back; every side profile is `[d, h]`:
 * - `rail`: the cradle's two floor rails, `a` from `in` to `out` on each
 *   side, the half length `d` and height `h`; `beams`: the two cross
 *   beams' centres along `d`, their half depth and height, reaching a
 *   centimetre into each rail; `saddle`: the plate under the neck between
 *   the rails, a little darker;
 * - `posts`: the cradle's uprights on the rails, their `d`, half side and
 *   heights; `clamp`: on each side a post at the neck's `d` and a bar
 *   from it in to the neck, `a` from `in` to `out`, half depth and
 *   heights;
 * - `neck`: the neck's centre along `d`, its radius and heights, from
 *   the saddle up into the skull;
 * - `hoses`: `[side, d, h]` for each hose: it runs level from inside the
 *   neck (`hoseIn`) out to the rail at `a = side * hoseOut` at height
 *   `h`, then down into the rail; `hoseR` its radius;
 * - `skull`: a row of overlapping squashed balls `[a, d, h, r, down,
 *   up]` (`ovoid`), from the face back, each smaller and higher than the
 *   last, so the helmet is long, narrow and swept back and up;
 * - `face`: the dark face plate, `a` half width, `d` and `h` ranges;
 *   `eyes`: the two narrow slanted eyes, `[a, h]` centre, length, width
 *   and slant (the outer end higher), a decal off the plate;
 * - `brow`: the hard brow ridge, a chevron seen from the front (`a`,
 *   `h`), from inside the skull (`d0`) out to `d1`, overhanging the
 *   eyes;
 * - `keel`: the forehead ridge the horn grows from, running back over the
 *   crown, and its half width;
 * - `horn`: the horn's points from the root in the keel just above the
 *   brow to the tip, which is the hero's top (the tip's `h` is replaced
 *   by `heroHalf`'s `top`); its width at each point and each segment's
 *   half thickness, so it curves forward and up and tapers; `band`: the
 *   lime band on the first segment, where along it and how long;
 * - `jaw`: the jaw guard's side profile, its upper edge stepped like
 *   teeth down to a point, `jawTop` and `jawLow` the upper and lower
 *   edges, `jawTip` the point; `jawSlices`: `[half width, tip d, grow]`,
 *   the narrower slices reaching further and standing `grow` proud at
 *   top and bottom, so the jaw narrows to its point without two faces in
 *   one plane; `jawTrim` the lime line along the lower edge;
 * - `cheek`: the cheek plates sweeping back and up on each side, their
 *   profile, their frame's turn outward, the offset from the centre
 *   line and their thickness; the lime line along their upper edge
 *   (`cheekTrim`, indices into the profile);
 * - `crest`: the hooked crest at the skull's back, its profile, half
 *   width and the indices its lime edge runs through; `trimW` the width
 *   of every lime line.
 */
const HEAD = {
  rail: { in: 0.5, out: 0.62, d: 0.9, h: 0.12 },
  beams: { at: [-0.75, 0.75], half: 0.05, h: 0.1 },
  saddle: { a: 0.5, d0: -0.55, d1: 0.35, h0: 0.04, h1: 0.12 },
  posts: { at: [-0.6, 0.35], half: 0.05, h: [0.12, 0.42] },
  clamp: {
    in: 0.22,
    out: 0.6,
    half: 0.06,
    h: [0.34, 0.44],
    post: [0.12, 0.48],
  },
  neck: { d: -0.12, r: 0.28, h: [0.12, 0.72] },
  hoses: [
    [1, -0.35, 0.24],
    [-1, 0.08, 0.26],
    [-1, -0.38, 0.21],
  ],
  hoseIn: 0.15,
  hoseOut: 0.56,
  hoseR: 0.04,
  skull: [
    [0, 0.08, 1.0, 0.3, 0.42, 0.36],
    [0, -0.2, 1.08, 0.33, 0.48, 0.36],
    [0, -0.48, 1.16, 0.28, 0.4, 0.3],
    [0, -0.7, 1.24, 0.2, 0.28, 0.22],
    [0, -0.86, 1.32, 0.12, 0.16, 0.14],
  ],
  face: { a: 0.24, d0: 0.2, d1: 0.4, h0: 0.8, h1: 1.02 },
  eyes: { at: 0.13, h: 0.93, length: 0.16, width: 0.04, slant: 0.26 },
  brow: {
    front: [
      [-0.3, 1.04],
      [0, 0.99],
      [0.3, 1.04],
      [0.3, 1.13],
      [0, 1.1],
      [-0.3, 1.13],
    ],
    d0: 0.22,
    d1: 0.5,
  },
  keel: [
    [0.48, 1.08],
    [0.48, 1.18],
    [0.05, 1.42],
    [-0.35, 1.47],
    [-0.35, 1.3],
    [0.2, 1.06],
  ],
  keelHalf: 0.08,
  horn: {
    pts: [
      [0.4, 1.14],
      [0.5, 1.45],
      [0.68, 1.78],
      [0.97, 0],
    ],
    widths: [0.14, 0.1, 0.07, 0],
    halves: [0.05, 0.04, 0.03],
  },
  band: { at: 0.45, length: 0.05 },
  jawTop: [
    [0.3, 0.8],
    [0.3, 0.76],
    [0.45, 0.74],
    [0.45, 0.7],
    [0.6, 0.66],
    [0.6, 0.62],
    [0.72, 0.58],
  ],
  jawLow: [
    [0.7, 0.44],
    [0.4, 0.46],
    [0.05, 0.56],
    [0.05, 0.82],
  ],
  jawTip: 0.46,
  jawSlices: [
    [0.22, 0.9, 0],
    [0.14, 0.95, 0.012],
    [0.07, 0.99, 0.024],
  ],
  cheek: {
    pts: [
      [0.22, 0.72],
      [0.2, 0.98],
      [-0.3, 1.2],
      [-0.72, 1.44],
      [-0.62, 1.2],
      [-0.3, 0.92],
      [0, 0.7],
    ],
    turn: (12 * Math.PI) / 180,
    at: 0.3,
    thick: 0.04,
  },
  cheekTrim: [1, 2, 3],
  crest: [
    [-0.6, 1.38],
    [-0.85, 1.33],
    [-0.98, 1.55],
    [-0.9, 1.82],
    [-0.78, 1.8],
    [-0.86, 1.58],
    [-0.7, 1.5],
  ],
  crestHalf: 0.05,
  crestEdge: [1, 2, 3],
  trimW: 0.03,
} as const;

/** The jaw's side profile with its tip at `tipD`, grown `grow` at top and bottom. */
function jawProfile(tipD: number, grow: number): P2[] {
  return [
    ...HEAD.jawTop.map(([d, h]): P2 => [d, h + grow]),
    [tipD, HEAD.jawTip],
    ...HEAD.jawLow.map(([d, h]): P2 => [d, h - grow]),
  ];
}

/**
 * The giant robot head: the cradle (rails, beams, saddle, posts,
 * clamps), the neck and hoses, the swept-back skull, the dark face and
 * the slanted eyes under the brow, the forehead keel and the horn with
 * its band, the stepped jaw guard, the cheek plates, the crest and the
 * lime trim.
 */
const mechHead: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { top } = heroHalf(kind, variant);
  const violet = s.tinted(HEAD_VIOLET);
  const lime = s.tinted(HEAD_LIME);
  const grey = s.tinted(CRADLE_GREY);
  const side = kitAt(sideways(ORIGIN));
  const L = DECAL_LIFT;
  const t = HEAD.trimW;
  const R = HEAD.rail;
  for (const sg of [-1, 1])
    k.box(sg * R.in, sg * R.out, -R.d, R.d, 0, R.h, grey);
  const Bm = HEAD.beams;
  for (const d of Bm.at)
    k.box(-R.in - 0.01, R.in + 0.01, d - Bm.half, d + Bm.half, 0, Bm.h, grey);
  const S = HEAD.saddle;
  k.box(-S.a, S.a, S.d0, S.d1, S.h0, S.h1, s.tinted(shade(CRADLE_GREY, 0.8)));
  const Po = HEAD.posts;
  const Cl = HEAD.clamp;
  const N = HEAD.neck;
  const mid = (R.in + R.out) / 2;
  for (const sg of [-1, 1]) {
    for (const d of Po.at)
      k.bevelBox(
        sg * mid - Po.half,
        sg * mid + Po.half,
        d - Po.half,
        d + Po.half,
        Po.h[0],
        Po.h[1],
        0.015,
        grey,
      );
    k.box(
      sg * mid - Po.half,
      sg * mid + Po.half,
      N.d - Cl.half,
      N.d + Cl.half,
      Cl.post[0],
      Cl.post[1],
      grey,
    );
    k.box(
      sg * Cl.in,
      sg * Cl.out,
      N.d - Cl.half + 0.01,
      N.d + Cl.half - 0.01,
      Cl.h[0],
      Cl.h[1],
      grey,
    );
  }
  k.cylinder(0, N.d, N.h[0], N.h[1], N.r, 12, s.tinted(NECK_DARK));
  const hose = s.tinted(HOSE_BLACK);
  for (const [sg, d, h] of HEAD.hoses) {
    const out = sg * HEAD.hoseOut;
    k.cylinderAlong(sg * HEAD.hoseIn, out, d, h, HEAD.hoseR, 6, hose);
    k.cylinder(out, d, R.h - 0.01, h, HEAD.hoseR, 6, hose);
  }
  HEAD.skull.forEach((b, i) =>
    ovoid(k, b, 10, i % 2 === 0 ? violet : s.tinted(shade(HEAD_VIOLET, 0.92))),
  );
  const F = HEAD.face;
  k.box(-F.a, F.a, F.d0, F.d1, F.h0, F.h1, s.tinted(HEAD_FACE));
  const E = HEAD.eyes;
  for (const [g, sg] of [
    [0, -1],
    [1, 1],
  ] as const)
    k.extrude(
      tiltedBar(sg * E.at, E.h, sg * E.slant, E.length, E.width),
      F.d1,
      F.d1 + L,
      s.blink(HEAD_EYE, g),
    );
  const Bw = HEAD.brow;
  k.extrude(Bw.front, Bw.d0, Bw.d1, s.tinted(shade(HEAD_VIOLET, 0.8)));
  const kh = HEAD.keelHalf;
  side.extrude(across(HEAD.keel), -kh, kh, violet);
  const Hn = HEAD.horn;
  const hornPts = Hn.pts.map(([d, h], i): P2 =>
    i === Hn.pts.length - 1 ? [d, top] : [d, h],
  );
  taperedSegments(hornPts, Hn.widths).forEach((seg, i) => {
    const half = Hn.halves[i] ?? 0.03;
    side.extrude(across(seg), -half, half, violet);
  });
  const [h0, h1] = hornPts;
  if (h0 && h1) {
    const Bd = HEAD.band;
    const len = Math.hypot(h1[0] - h0[0], h1[1] - h0[1]);
    const w = (Hn.widths[0] + Hn.widths[1]) / 2;
    const half = (Hn.halves[0] ?? 0.05) + L;
    side.extrude(
      across(
        barBetween(
          lerp2(h0, h1, Bd.at - Bd.length / len / 2),
          lerp2(h0, h1, Bd.at + Bd.length / len / 2),
          w + 2 * L,
        ),
      ),
      -half,
      half,
      lime,
    );
  }
  HEAD.jawSlices.forEach(([w, tipD, grow], i) => {
    side.extrude(across(jawProfile(tipD, grow)), -w, w, violet);
    if (i > 0) return;
    const low: P2[] = [[tipD, HEAD.jawTip], ...HEAD.jawLow.slice(0, 3)];
    for (let j = 0; j + 1 < low.length; j++) {
      const p = low[j];
      const q = low[j + 1];
      if (!p || !q) continue;
      for (const sg of [-1, 1])
        side.extrude(across(barBetween(p, q, t)), sg * w, sg * (w + L), lime);
    }
  });
  const Ch = HEAD.cheek;
  for (const sg of [-1, 1]) {
    const cheek = kitAt(sideways(yawed(ORIGIN, 0, 0, sg * Ch.turn)));
    const [c0, c1] = [sg * Ch.at, sg * (Ch.at + Ch.thick)];
    cheek.extrude(across(Ch.pts), c0, c1, violet);
    const edge = HEAD.cheekTrim.map((i) => Ch.pts[i] ?? Ch.pts[0]);
    for (let j = 0; j + 1 < edge.length; j++) {
      const p = edge[j];
      const q = edge[j + 1];
      if (!p || !q) continue;
      cheek.extrude(across(barBetween(p, q, t)), c1, c1 + sg * L, lime);
    }
  }
  const ch = HEAD.crestHalf;
  side.extrude(across(HEAD.crest), -ch, ch, violet);
  const edge = HEAD.crestEdge.map((i) => HEAD.crest[i] ?? HEAD.crest[0]);
  for (let i = 0; i + 1 < edge.length; i++) {
    const p = edge[i];
    const q = edge[i + 1];
    if (!p || !q) continue;
    for (const sg of [-1, 1])
      side.extrude(across(barBetween(p, q, t)), sg * ch, sg * (ch + L), lime);
  }
};

/** The tank's body: a pale sky blue. */
const TANK_BLUE: Rgb = [0.55, 0.75, 0.95];

/** The tank's joints and arms: white. */
const TANK_WHITE: Rgb = [0.92, 0.93, 0.95];

/** The tank's underparts and wheels: a darker grey. */
const TANK_GREY: Rgb = [0.3, 0.32, 0.35];

/** The lenses' bezels: near black. */
const TANK_BEZEL: Rgb = [0.08, 0.08, 0.1];

/** The lenses' glow: a pale cyan that breathes. */
const TANK_LENS: Rgb = [0.5, 0.9, 1.0];

/**
 * The tank's three lenses on its cabin's front: `a` and `h` the centre,
 * `r` the bezel's radius. Two side by side on top and a larger one below
 * them (the lowest lens is the largest, which the shape test holds). The
 * recipe builds its lenses from this table, each glowing in its own blink
 * group in this order.
 */
export const TANK_LENSES: readonly { a: number; h: number; r: number }[] = [
  { a: -0.18, h: 1.62, r: 0.09 },
  { a: 0.18, h: 1.62, r: 0.09 },
  { a: 0, h: 1.35, r: 0.13 },
];

/**
 * The spider tank's body measures, in metres (the footprint is ±1.2 by
 * ±1.75; its top is the catalogue's, read through `heroHalf`):
 * - `pods`: the abdomen's two ovoids, `[d, scale]`: the full one at
 *   `d -0.7` and one at 0.85 of its size behind it, both on the `pod`
 *   profile of `[r, h]` closed at the hero's top (`heroHalf`'s `top`) on
 *   its axis (the smaller one scaled about the profile's foot) and the
 *   smaller a shade darker;
 * - `waist`, `cabin`, `under`: the white waist cylinder, the cabin's
 *   bevelled box and the grey underparts;
 * - `lens`: the bezels' depth range, from inside the cabin to in front
 *   of it, so where the cabin's bevel falls away a bezel stands out as a
 *   short barrel; the glowing discs' front in front of the bezels and
 *   their share of the radius;
 * - `arms`: the manipulator arms' centres along `a`, half width, `d` and
 *   `h` ranges (their tops inside the cabin), and `claws` each arm's two
 *   claw bars at its tip, `spread` either side of its middle;
 * - `gun`: the gun pod's box, sunk a centimetre into the cabin's roof,
 *   and its barrel.
 */
const TANK = {
  pods: [
    [-0.7, 1],
    [-1.05, 0.85],
  ],
  pod: [
    [0, 1.1],
    [0.55, 1.2],
    [0.8, 1.6],
    [0.7, 2.2],
    [0.35, 2.45],
  ],
  waist: { d: -0.05, r: 0.32, h: [1.15, 1.7] },
  cabin: { a: 0.45, d0: 0.3, d1: 1.2, h0: 1.1, h1: 1.8, bevel: 0.16 },
  under: { a: 0.5, d0: -1.0, d1: 0.6, h0: 0.9, h1: 1.12 },
  lens: { d0: 1.1, d1: 1.24, glow: 1.25, share: 0.7 },
  arms: { at: [-0.2, 0.2], half: 0.04, d0: 0.9, d1: 1.38, h0: 0.98, h1: 1.12 },
  claws: { half: 0.015, d0: 1.3, d1: 1.42, h0: 0.9, h1: 1.0, spread: 0.025 },
  gun: { a: 0.1, d0: 0.4, d1: 1.0, h0: 1.79, h1: 1.95 },
  barrel: { a: 0.03, d0: 1.0, d1: 1.3, h0: 1.85, h1: 1.9 },
} as const;

/**
 * The tank's legs, in each leg's own frame (`yawed(ORIGIN, 0, d, turn)`,
 * so its `a` runs out along the leg and its `h` up):
 * - `hips`: `[d, turn]` for the four legs: the frame's origin on the
 *   centre line at `d` and its turn from `+a`, the front pair splayed
 *   forward and the back pair back, each wheel inside the footprint;
 * - `hip`, `knee`, `foot`: the joints' `[a, h]` along the leg; the upper
 *   leg runs from hip to knee, `upper` wide, the lower leg from knee to
 *   foot, `lower` wide, both `half` thick;
 * - `joint`: the white joint pins' radius and half length across the
 *   leg; `wheel`: the wheel's radius (its lowest corner on the floor, the
 *   outline starting at its top so a corner points straight down), half
 *   width and facets; `hub`: the white hub's radius and how far it
 *   stands off each face.
 */
const TANK_LEGS = {
  hips: [
    [0.4, (45 * Math.PI) / 180],
    [0.4, Math.PI - (45 * Math.PI) / 180],
    [-0.6, -(39 * Math.PI) / 180],
    [-0.6, Math.PI + (39 * Math.PI) / 180],
  ],
  hip: [0.3, 1.3],
  knee: [0.95, 1.75],
  foot: [1.35, 0.24],
  upper: 0.18,
  lower: 0.14,
  half: 0.09,
  joint: { r: 0.11, half: 0.12 },
  wheel: { r: 0.24, half: 0.08, sides: 10 },
  hub: { r: 0.1, off: 0.015 },
} as const;

/**
 * A frame at `a` along frame `f`, facing as the recipe's own frame does:
 * a leg's foot, where the wheel is built square to the tank.
 */
function footFrame(f: Frame, a: number): Frame {
  return {
    origin: [
      f.origin[0] + f.along[0] * a,
      f.origin[1],
      f.origin[2] + f.along[2] * a,
    ],
    along: [...ORIGIN.along],
    inward: [...ORIGIN.inward],
  };
}

/**
 * One of the tank's legs in frame `f`: the blue upper leg from the white
 * hip pin to the white knee pin, the blue lower leg down to the wheel,
 * and the grey wheel with a white hub on each face, turned to roll along
 * `d` and resting on the floor.
 */
function tankLeg(kitAt: KitAt, s: Surfaces, f: Frame): void {
  const L = TANK_LEGS;
  const k = kitAt(f);
  const blue = s.tinted(TANK_BLUE);
  const white = s.tinted(TANK_WHITE);
  k.extrude(barBetween(L.hip, L.knee, L.upper), -L.half, L.half, blue);
  k.extrude(barBetween(L.knee, L.foot, L.lower), -L.half, L.half, blue);
  // A pin across the leg: in the leg's sideways frame `cylinderAlong`
  // runs along the leg's `d`, with the leg's `a` as its depth.
  const pins = kitAt(sideways(f));
  for (const [a, h] of [L.hip, L.knee])
    pins.cylinderAlong(-L.joint.half, L.joint.half, a, h, L.joint.r, 8, white);
  const [fa, fh] = L.foot;
  const wheel = kitAt(sideways(footFrame(f, fa)));
  const W = L.wheel;
  wheel.extrude(
    discOutline(0, fh, W.r, W.sides, "top"),
    -W.half,
    W.half,
    s.tinted(TANK_GREY),
  );
  for (const sg of [-1, 1])
    wheel.extrude(
      discOutline(0, fh, L.hub.r, 8, "top"),
      sg * W.half,
      sg * (W.half + L.hub.off),
      white,
    );
}

/**
 * The spider tank: the abdomen's two ovoids, the waist, the cabin, the
 * underparts, the three lenses, the arms and claws, the gun pod and the
 * four legs.
 */
const spiderTank: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { top } = heroHalf(kind, variant);
  const blue = s.tinted(TANK_BLUE);
  const white = s.tinted(TANK_WHITE);
  const grey = s.tinted(TANK_GREY);
  const foot = TANK.pod[0][1];
  for (const [d, x] of TANK.pods)
    k.lathe(
      0,
      d,
      [...TANK.pod, [0, top] as const].map(
        ([r, h]) => [r * x, foot + (h - foot) * x] as const,
      ),
      12,
      x === 1 ? blue : s.tinted(shade(TANK_BLUE, 0.94)),
    );
  const W = TANK.waist;
  k.cylinder(0, W.d, W.h[0], W.h[1], W.r, 10, white);
  const C = TANK.cabin;
  k.bevelBox(-C.a, C.a, C.d0, C.d1, C.h0, C.h1, C.bevel, blue);
  const U = TANK.under;
  k.box(-U.a, U.a, U.d0, U.d1, U.h0, U.h1, grey);
  const bezel = s.tinted(TANK_BEZEL);
  const Ln = TANK.lens;
  TANK_LENSES.forEach(({ a, h, r }, g) => {
    k.extrude(discOutline(a, h, r, 10), Ln.d0, Ln.d1, bezel);
    k.extrude(
      discOutline(a, h, Ln.share * r, 10),
      Ln.d1,
      Ln.glow,
      s.blink(TANK_LENS, g),
    );
  });
  const A = TANK.arms;
  const Cl = TANK.claws;
  for (const a of A.at) {
    k.box(a - A.half, a + A.half, A.d0, A.d1, A.h0, A.h1, white);
    for (const sg of [-1, 1]) {
      const c = a + sg * Cl.spread;
      k.box(c - Cl.half, c + Cl.half, Cl.d0, Cl.d1, Cl.h0, Cl.h1, white);
    }
  }
  const G = TANK.gun;
  k.box(-G.a, G.a, G.d0, G.d1, G.h0, G.h1, grey);
  const Br = TANK.barrel;
  k.box(-Br.a, Br.a, Br.d0, Br.d1, Br.h0, Br.h1, grey);
  for (const [d, turn] of TANK_LEGS.hips)
    tankLeg(kitAt, s, yawed(ORIGIN, 0, d, turn));
};

/** The robot's rust: a weathered red-brown. */
const ROBOT_RUST: Rgb = [0.45, 0.28, 0.18];

/** The robot's darker rust: legs, feet, plates, joints and arm cores. */
const ROBOT_DARK: Rgb = [0.3, 0.18, 0.12];

/** The moss on its top and shoulders. */
const ROBOT_MOSS: Rgb = [0.32, 0.5, 0.2];

/** The eye socket and the crack: near black. */
const ROBOT_SOCKET: Rgb = [0.07, 0.05, 0.04];

/** The eye: a faint amber that breathes. */
const ROBOT_EYE: Rgb = [1.0, 0.65, 0.2];

/** The bird: a small blue songbird. */
const BIRD_BLUE: Rgb = [0.25, 0.45, 0.8];

/** The bird's beak: yellow-orange. */
const BIRD_BEAK: Rgb = [0.95, 0.65, 0.15];

/** The flowers' stems: a fresh green. */
const STEM_GREEN: Rgb = [0.3, 0.6, 0.2];

/** The flowers' heads: yellow, pink and yellow. */
const FLOWER_TINTS: readonly Rgb[] = [
  [0.98, 0.85, 0.2],
  [0.95, 0.5, 0.7],
  [0.98, 0.85, 0.2],
];

/**
 * The garden robot's measures, in metres (the footprint is ±0.9 by
 * ±0.75; its top is the catalogue's, read through `heroHalf`):
 * - `legs`: the two stubby block legs, `a` from `in` to `out`, their half
 *   depth, heights and bevel; `feet`: their flat feet, `a` from `in` to
 *   `out`, `d` and `h` ranges;
 * - `body`: the body's profile `[r, h]`, broad at the shoulders and
 *   narrowing to the hips, split into `bands` (`[first, last, shade]`
 *   index ranges that share their end rings) each in its own shade of
 *   rust, so the body reads weathered, not moulded;
 * - `plates`: the ten dark plates round the middle, each in a frame
 *   turned by a tenth of a turn: half width, `d` and `h` ranges;
 * - `shoulder`: the round shoulder joints' `a` and height, and their
 *   profile `[r, dh]` round that height;
 * - `arm`: the shoulder, elbow and wrist `[a, h]` (on the `+a` side,
 *   mirrored for `-a`); the dark core runs the whole length, `core` wide
 *   and `coreHalf` thick, and the flat rust segments on it, `width` wide
 *   and `half` thick, `upper` and `lower` of them on the two parts with a
 *   `gap` between; `elbowR` the elbow ball's radius;
 * - `palm`: the hand's block round the wrist, `below` it and `above` it;
 *   `fingers`: the three fingers' `d` and half side, from `grip` inside
 *   the palm down to `reach` below the wrist, ten centimetres off the
 *   floor. The hand hangs from the wrist, so a higher wrist lifts it;
 * - `neck` and `head`: the thin neck from inside the body, the domed
 *   head's profile `[r, h]` and the `knob` on top, up to the hero's top
 *   (`heroHalf`'s `top`);
 * - `face`: the lighter face plate's radius and depth range; `eye`: the
 *   socket's and the glowing eye's radius, height and depth ranges;
 * - `moss`: patches draped over the upper body, `[turn, half width, h0,
 *   h1, shade]` (each follows the body's profile from `h0` to `h1` in
 *   `steps`, `thick` proud and `sink` inside it, in its own shade of
 *   moss), and a cap on each shoulder, its profile `[r, dh]` round the
 *   shoulder's height;
 * - `bird`: where on the right shoulder's cap it perches;
 * - `crack`: the dark crack on the body's front, and `flowers`: `[a,
 *   top]` for each stem from the crack, its head `flowerHead` on a side.
 */
const ROBOT = {
  legs: { in: 0.12, out: 0.42, half: 0.16, h: [0.08, 0.7], bevel: 0.04 },
  feet: { in: 0.08, out: 0.48, d0: -0.22, d1: 0.3, h0: 0, h1: 0.1 },
  body: [
    [0, 0.62],
    [0.3, 0.64],
    [0.42, 0.85],
    [0.5, 1.15],
    [0.62, 1.6],
    [0.68, 2.0],
    [0.64, 2.3],
    [0.48, 2.52],
    [0.22, 2.62],
    [0, 2.64],
  ],
  bands: [
    [0, 2, 0.8],
    [2, 4, 0.95],
    [4, 5, 1.06],
    [5, 6, 1.0],
    [6, 9, 0.9],
  ],
  plates: { half: 0.17, d0: 0.58, d1: 0.68, h0: 1.5, h1: 1.75 },
  shoulder: {
    a: 0.66,
    h: 2.12,
    ball: [
      [0, -0.13],
      [0.09, -0.1],
      [0.13, 0],
      [0.09, 0.1],
      [0, 0.13],
    ],
  },
  arm: {
    shoulder: [0.72, 2.05],
    elbow: [0.8, 1.3],
    wrist: [0.82, 0.42],
    core: 0.05,
    coreHalf: 0.04,
    width: 0.1,
    half: 0.06,
    upper: 3,
    lower: 3,
    gap: 0.04,
    elbowR: 0.07,
  },
  palm: { a: 0.05, d: 0.09, below: 0.12, above: 0.04 },
  fingers: { d: [-0.065, 0, 0.065], half: 0.018, grip: 0.03, reach: 0.32 },
  neck: { r: 0.07, h: [2.58, 2.84] },
  head: [
    [0, 2.8],
    [0.22, 2.8],
    [0.27, 2.87],
    [0.28, 3.0],
    [0.26, 3.15],
    [0.19, 3.28],
    [0.09, 3.35],
    [0, 3.36],
  ],
  knob: { r: 0.04, h0: 3.35 },
  face: { r: 0.17, d: [0.22, 0.28] },
  eye: { h: 3.04, socket: 0.1, glow: 0.06, d: [0.26, 0.29, 0.3] },
  moss: {
    patches: [
      [0.3, 0.22, 2.05, 2.6, 1.0],
      [1.3, 0.16, 2.25, 2.6, 0.85],
      [2.2, 0.24, 2.0, 2.58, 1.1],
      [3.2, 0.18, 2.2, 2.6, 0.9],
      [4.1, 0.22, 2.1, 2.56, 1.05],
      [5.2, 0.17, 2.3, 2.6, 0.8],
    ],
    steps: 3,
    thick: 0.02,
    sink: 0.05,
    cap: [
      [0.14, 0.02],
      [0.1, 0.11],
      [0, 0.145],
    ],
  },
  bird: { a: 0.66, d: 0.0 },
  crack: { a: 0.02, d0: 0.63, d1: 0.675, h0: 1.8, h1: 2.02 },
  flowers: [
    [-0.035, 2.05],
    [0.02, 2.1],
    [0.06, 2.01],
  ],
  flowerHead: 0.028,
} as const;

/** The body's radius at height `h`, read off `ROBOT.body` by straight lines. */
function bodyRadius(h: number): number {
  const b = ROBOT.body;
  for (let i = 0; i + 1 < b.length; i++) {
    const [r0, h0] = b[i] ?? [0, 0];
    const [r1, h1] = b[i + 1] ?? [0, 0];
    if (h >= h0 && h <= h1) return r0 + ((r1 - r0) * (h - h0)) / (h1 - h0);
  }
  return 0;
}

/**
 * One arm's run from `p` to `q` on the `sg` side: a dark core over the
 * whole run and `n` flat rust segments on it with a `gap` between them.
 */
function armRun(
  kitAt: KitAt,
  s: Surfaces,
  p: P2,
  q: P2,
  n: number,
  shadeOf: (i: number) => number,
): void {
  const A = ROBOT.arm;
  const k = kitAt(ORIGIN);
  k.extrude(
    barBetween(p, q, A.core),
    -A.coreHalf,
    A.coreHalf,
    s.tinted(ROBOT_DARK),
  );
  const len = Math.hypot(q[0] - p[0], q[1] - p[1]);
  const g = A.gap / len;
  for (let i = 0; i < n; i++) {
    const a = lerp2(p, q, i / n + g / 2);
    const b = lerp2(p, q, (i + 1) / n - g / 2);
    k.extrude(
      barBetween(a, b, A.width),
      -A.half,
      A.half,
      s.tinted(shade(ROBOT_RUST, shadeOf(i))),
    );
  }
}

/**
 * The garden robot: legs and feet, the banded body, the plates, the
 * shoulders and arms with their hands, the neck, the head with its face
 * and eye, the moss, the bird and the flowers in their crack.
 */
const gardenRobot: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { top } = heroHalf(kind, variant);
  const rust = (x: number) => s.tinted(shade(ROBOT_RUST, x));
  const dark = s.tinted(ROBOT_DARK);
  const moss = s.tinted(ROBOT_MOSS);
  const Lg = ROBOT.legs;
  const Ft = ROBOT.feet;
  for (const sg of [-1, 1]) {
    k.bevelBox(
      sg * Lg.in,
      sg * Lg.out,
      -Lg.half,
      Lg.half,
      Lg.h[0],
      Lg.h[1],
      Lg.bevel,
      dark,
    );
    k.box(sg * Ft.in, sg * Ft.out, Ft.d0, Ft.d1, Ft.h0, Ft.h1, dark);
  }
  for (const [i0, i1, x] of ROBOT.bands)
    k.lathe(0, 0, ROBOT.body.slice(i0, i1 + 1), 14, rust(x));
  const P = ROBOT.plates;
  for (let j = 0; j < 10; j++)
    kitAt(yawed(ORIGIN, 0, 0, (j * Math.PI) / 5)).box(
      -P.half,
      P.half,
      P.d0,
      P.d1,
      P.h0,
      P.h1,
      dark,
    );
  const Sh = ROBOT.shoulder;
  const A = ROBOT.arm;
  const Pm = ROBOT.palm;
  const Fg = ROBOT.fingers;
  const M = ROBOT.moss;
  for (const sg of [-1, 1]) {
    const round = (pts: readonly P2[]) =>
      pts.map(([r, dh]) => [r, Sh.h + dh] as const);
    k.lathe(sg * Sh.a, 0, round(Sh.ball), 6, rust(0.85));
    k.lathe(sg * Sh.a, 0, round(M.cap), 6, moss);
    const m = (p: P2): P2 => [sg * p[0], p[1]];
    armRun(
      kitAt,
      s,
      m(A.shoulder),
      m(A.elbow),
      A.upper,
      (i) => 0.95 - 0.06 * i,
    );
    armRun(
      kitAt,
      s,
      m(A.elbow),
      m(A.wrist),
      A.lower,
      (i) => 0.9 - 0.05 * (i % 2),
    );
    const [ea, eh] = A.elbow;
    k.lathe(
      sg * ea,
      0,
      [
        [0, eh - A.elbowR],
        [A.elbowR, eh - A.elbowR / 2],
        [A.elbowR, eh + A.elbowR / 2],
        [0, eh + A.elbowR],
      ],
      6,
      dark,
    );
    const [wa0, wh] = A.wrist;
    const wa = sg * wa0;
    const palm0 = wh - Pm.below;
    k.box(wa - Pm.a, wa + Pm.a, -Pm.d, Pm.d, palm0, wh + Pm.above, dark);
    for (const d of Fg.d)
      k.box(
        wa - Fg.half,
        wa + Fg.half,
        d - Fg.half,
        d + Fg.half,
        wh - Fg.reach,
        palm0 + Fg.grip,
        rust(0.75),
      );
  }
  const Nk = ROBOT.neck;
  k.cylinder(0, 0, Nk.h[0], Nk.h[1], Nk.r, 8, dark, false);
  k.lathe(0, 0, ROBOT.head, 10, rust(1.0));
  const Kn = ROBOT.knob;
  k.cylinder(0, 0, Kn.h0, top, Kn.r, 6, dark);
  const E = ROBOT.eye;
  const Fc = ROBOT.face;
  k.extrude(discOutline(0, E.h, Fc.r, 10), Fc.d[0], Fc.d[1], rust(1.35));
  const [ed0, ed1, ed2] = E.d;
  k.extrude(
    discOutline(0, E.h, E.socket, 10),
    ed0,
    ed1,
    s.tinted(ROBOT_SOCKET),
  );
  k.extrude(discOutline(0, E.h, E.glow, 10), ed1, ed2, s.blink(ROBOT_EYE, 0));
  // The moss patches: strips that follow the body's upper profile, each
  // in a frame turned to its patch, so its `a` runs out from the axis.
  for (const [turn, w, h0, h1, x] of M.patches) {
    const n = M.steps;
    const hs = Array.from(
      { length: n + 1 },
      (_, i) => h0 + ((h1 - h0) * i) / n,
    );
    const outer = hs.map((h): [number, number] => [bodyRadius(h) + M.thick, h]);
    const inner = hs
      .map((h): [number, number] => [bodyRadius(h) - M.sink, h])
      .reverse();
    kitAt(yawed(ORIGIN, 0, 0, turn)).extrude(
      [...outer, ...inner],
      -w,
      w,
      s.tinted(shade(ROBOT_MOSS, x)),
    );
  }
  // The bird on the right shoulder's cap.
  const Bd = ROBOT.bird;
  const b0 = Sh.h + (M.cap[2]?.[1] ?? 0);
  const blue = s.tinted(BIRD_BLUE);
  k.lathe(
    Bd.a,
    Bd.d,
    [
      [0, b0 - 0.01],
      [0.035, b0 + 0.005],
      [0.05, b0 + 0.04],
      [0.03, b0 + 0.075],
      [0, b0 + 0.085],
    ],
    6,
    blue,
  );
  k.lathe(
    Bd.a,
    Bd.d + 0.035,
    [
      [0, b0 + 0.06],
      [0.028, b0 + 0.08],
      [0.022, b0 + 0.105],
      [0, b0 + 0.11],
    ],
    6,
    blue,
  );
  k.box(
    Bd.a - 0.008,
    Bd.a + 0.008,
    Bd.d + 0.055,
    Bd.d + 0.085,
    b0 + 0.08,
    b0 + 0.092,
    s.tinted(BIRD_BEAK),
  );
  k.box(
    Bd.a - 0.015,
    Bd.a + 0.015,
    Bd.d - 0.1,
    Bd.d - 0.03,
    b0 + 0.02,
    b0 + 0.035,
    blue,
  );
  // The crack and the flowers growing out of it.
  const Cr = ROBOT.crack;
  k.box(-Cr.a, Cr.a, Cr.d0, Cr.d1, Cr.h0, Cr.h1, s.tinted(ROBOT_SOCKET));
  const stem = s.tinted(STEM_GREEN);
  const fh = ROBOT.flowerHead;
  ROBOT.flowers.forEach(([a, top], i) => {
    k.box(
      a - 0.006,
      a + 0.006,
      Cr.d1 - 0.015,
      Cr.d1 + 0.008,
      Cr.h0 + 0.1,
      top,
      stem,
    );
    const tint = FLOWER_TINTS[i] ?? ROBOT_MOSS;
    k.box(
      a - fh,
      a + fh,
      Cr.d1 - 0.012,
      Cr.d1 + 0.025,
      top - 0.005,
      top + fh,
      s.tinted(tint),
    );
  });
};

/** The machine kinds' recipes. */
export const MECH_RECIPES = {
  "mech-head": mechHead,
  "spider-tank": spiderTank,
  "garden-robot": gardenRobot,
} satisfies Record<
  Extract<HeroKind, "mech-head" | "spider-tank" | "garden-robot">,
  HeroRecipe
>;
