/**
 * The exhibits' recipes: the stone hand, the moon rocket and the thunder
 * hammer. What they share is that each is one striking object standing
 * free in the band, like a piece in a museum: pure mass with no light and
 * no motion (their bank is `steady`, C13). The hand and the rocket stand
 * on low plinths; the hammer lies on the floor where it fell, in a small
 * crack of its own.
 *
 * - The hand is a craggy brick-red stone right fist, twice human size,
 *   upright on a grey plinth: a broken edge where the forearm meets the
 *   plinth, faceted slabs of slightly different stone with dark grooves
 *   between them and dark cracks on their faces, four thick fingers
 *   curled on its front (`+d`) under big square knuckles, and the thumb
 *   folded across them from its `+a` side.
 * - The rocket is a slim red and white chequered hull (an octagonal prism
 *   of panels, `ROCKET`), a white tail cone and a red nose, standing on
 *   three swept fins on a low round plinth, one fin at its back and two
 *   spread at its front, with a row of portholes near the top facing `+d`.
 * - The hammer's grey bevelled head is the screen prop's true size
 *   (`HAMMER_HEAD`), its long axis along `a`, flat on the floor, with two
 *   faint knotwork bands, and a band of runes cut into each long side
 *   (`+d` and `-d`) in the panel between them, in a grey darker than the
 *   knotwork (`RUNE_INK`). Its leather-wrapped handle rises from the head's
 *   top middle towards `-d` at 55 degrees and ends in a pommel with a
 *   strap hanging from it. Under the head a dark dent marks the floor,
 *   and flat jagged cracks a few millimetres high radiate from it,
 *   wide at the root and narrowing to a point, a few of them forking;
 *   the head is the hammer's one `bevelBox`.
 *
 * The numbers each kind is built to are named in a table above its
 * recipe: `HAND`, `ROCKET`, and `HAMMER_HEAD` with `HAMMER`.
 */

import type { HeroKind } from "../../../world/types";
import { DECAL_LIFT, frameAt, type Frame } from "../../kit";
import type { Rgb } from "../../looks";
import { discOutline, shade, sideways, tiltedBar, yawed } from "../common";
import { HAMMER_RUNES } from "../marks";
import { heroHalf, type HeroRecipe } from "./common";
import { fit, pixelPanel } from "./pixels";

/** The recipe's own frame: the origin, facing north. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/** The hand's plinth: a dark stone grey. */
const HAND_PLINTH: Rgb = [0.3, 0.3, 0.32];

/** The hand's brick-red stone. */
const STONE_RED: Rgb = [0.55, 0.16, 0.12];

/** The dark red-brown in the hand's cracks. */
const STONE_CRACK: Rgb = [0.3, 0.1, 0.08];

/**
 * The stone hand's measures, in metres. Every block is
 * `[a0, a1, d0, d1, h0, h1, bevel, shade]`: its ranges along `a`, `d` and
 * `h`, its bevel and how light its stone is (`shade` of `STONE_RED`), so
 * neighbouring slabs differ a little and the facets read as separate
 * stones, not one moulded shell.
 * - `plinth`: the plinth's height and bevel (it fills the footprint);
 * - `base`: the broken edge, five irregular slabs round the forearm's
 *   foot, each `[a0, a1, d0, d1, h1]` from the plinth's top up to `h1`,
 *   in the `baseShade` stone;
 * - `blocks`: the forearm, its uneven faceting slabs (two on the front
 *   with a groove between them, one on each side, two at the back), the
 *   darker recessed wrist, the fist wider than the forearm so the hand
 *   reads as the big end, a slab on the fist's back, and the thumb: its
 *   ball on the `+a` side, the thumb folded across the fingers and its
 *   tip tucked in at `-a`;
 * - `chips`: broken lumps on two of the forearm's top corners (back
 *   `-a` and front `+a`), each standing a little proud of the faces it
 *   sits on so no face of it lies in one of the forearm's, which breaks
 *   the straight edges;
 * - `fingers`: `[a0, a1, front, knuckleTop, shade, knuckleShade,
 *   knuckleFront]`, index at `+a` to little finger at `-a`. A finger is
 *   a block from `fingerD0` to its `front` over `fingerH`, bevelled by
 *   `fingerBevel`, with a dark crease across its front over `crease` (the
 *   joint of the curled finger) and a dark line under its knuckle over
 *   `underKnuckle`; its big square knuckle runs from `knuckleD0` to
 *   `knuckleFront`, from `knuckleH` up to `knuckleTop`, bevelled by
 *   `knuckleBevel`. The fronts differ by millimetres, so the row is not
 *   ruler straight;
 * - `gaps`: the `a` ranges of the dark grooves that fill the gaps between
 *   fingers and between knuckles, each a little wider than its gap so its
 *   sides hide in the stone, set back from the fronts (`gapFront` and
 *   `gapKnuckleFront`) so it reads as a deep crack;
 * - `grooves`: other dark recesses `[a0, a1, d0, d1, h0, h1]`, here the
 *   one between the two front slabs;
 * - `cracks`: dark bars `[a0, a1, d0, d1, h0, h1]` laid `DECAL_LIFT`
 *   proud on the flat faces of the forearm, the slabs and the fist.
 * There are no carved marks, runes or text. Everything fits inside the
 * 0.45 half width and under the 1.4 top.
 */
const HAND = {
  plinth: { h: 0.3, bevel: 0.03 },
  base: [
    [-0.14, -0.02, 0.15, 0.2, 0.4],
    [0.04, 0.16, 0.15, 0.19, 0.36],
    [0.17, 0.23, -0.1, 0.03, 0.42],
    [-0.05, 0.08, -0.21, -0.15, 0.38],
    [-0.24, -0.17, -0.02, 0.11, 0.39],
  ],
  baseShade: 0.72,
  blocks: [
    // The forearm and its slabs.
    [-0.2, 0.2, -0.17, 0.17, 0.3, 0.85, 0.04, 1.0],
    [-0.165, -0.005, 0.17, 0.2, 0.44, 0.8, 0.02, 0.74],
    [0.005, 0.16, 0.17, 0.19, 0.5, 0.77, 0.015, 0.94],
    [0.2, 0.225, -0.13, 0.06, 0.4, 0.7, 0.012, 0.78],
    [-0.222, -0.2, -0.05, 0.13, 0.5, 0.82, 0.012, 0.9],
    [-0.13, 0.13, -0.2, -0.17, 0.5, 0.78, 0.02, 0.86],
    [-0.1, 0.06, -0.19, -0.17, 0.36, 0.49, 0.015, 0.74],
    // The wrist, the fist and the slab on its back.
    [-0.17, 0.17, -0.15, 0.15, 0.85, 0.95, 0.03, 0.66],
    [-0.25, 0.25, -0.15, 0.12, 0.95, 1.3, 0.04, 1.0],
    [-0.2, 0.08, -0.165, -0.15, 1.0, 1.24, 0.01, 0.84],
    // The thumb's ball, the thumb and its tip.
    [0.16, 0.27, 0.08, 0.26, 0.95, 1.13, 0.03, 0.82],
    [-0.1, 0.25, 0.21, 0.29, 1.0, 1.13, 0.02, 1.12],
    [-0.16, -0.09, 0.19, 0.27, 1.0, 1.11, 0.02, 0.9],
  ],
  chips: [
    [-0.205, -0.14, -0.175, -0.12, 0.79, 0.855, 0.01, 0.74],
    [0.12, 0.205, 0.13, 0.205, 0.79, 0.86, 0.012, 0.9],
  ],
  fingers: [
    [0.127, 0.245, 0.212, 1.4, 1.02, 0.84, 0.222],
    [0.003, 0.121, 0.216, 1.4, 0.82, 0.76, 0.226],
    [-0.121, -0.003, 0.209, 1.385, 1.08, 0.9, 0.218],
    [-0.245, -0.127, 0.205, 1.36, 0.86, 0.8, 0.214],
  ],
  fingerD0: 0.12,
  fingerH: [1.0, 1.28],
  fingerBevel: 0.015,
  knuckleD0: 0.02,
  knuckleH: 1.26,
  knuckleBevel: 0.025,
  crease: [1.16, 1.172],
  underKnuckle: [1.245, 1.257],
  gaps: [
    [0.119, 0.129],
    [-0.005, 0.005],
    [-0.129, -0.119],
  ],
  gapFront: 0.2,
  gapKnuckleFront: 0.21,
  gapKnuckleTop: 1.36,
  grooves: [[-0.008, 0.008, 0.17, 0.185, 0.46, 0.78]],
  cracks: [
    // The +a side slab, and the forearm's +a face under it.
    [0.225, 0.235, -0.03, -0.018, 0.43, 0.67],
    [0.2, 0.21, -0.12, 0.1, 0.36, 0.372],
    // The forearm's -a face.
    [-0.21, -0.2, -0.12, -0.108, 0.36, 0.8],
    [-0.21, -0.2, -0.12, 0.12, 0.42, 0.432],
    // The two front slabs.
    [-0.1, -0.088, 0.2, 0.21, 0.48, 0.77],
    [0.025, 0.14, 0.19, 0.2, 0.62, 0.632],
    // The fist's back slab and its -a side.
    [-0.04, -0.028, -0.175, -0.165, 1.02, 1.22],
    [-0.26, -0.25, -0.1, 0.08, 1.12, 1.132],
  ],
} as const;

/**
 * The stone hand: plinth, broken base, the blocks (forearm, slabs, wrist,
 * fist, thumb) each in its own shade of stone, the chipped corners, the
 * fingers and knuckles with the dark grooves between them, and the
 * cracks.
 */
const stoneHand: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const stone = (x: number) => s.tinted(shade(STONE_RED, x));
  const crack = s.tinted(STONE_CRACK);
  k.bevelBox(
    -hw,
    hw,
    -hw,
    hw,
    0,
    HAND.plinth.h,
    HAND.plinth.bevel,
    s.tinted(HAND_PLINTH),
  );
  const base = stone(HAND.baseShade);
  for (const [a0, a1, d0, d1, h1] of HAND.base)
    k.box(a0, a1, d0, d1, HAND.plinth.h, h1, base);
  for (const [a0, a1, d0, d1, h0, h1, bevel, x] of [
    ...HAND.blocks,
    ...HAND.chips,
  ])
    k.bevelBox(a0, a1, d0, d1, h0, h1, bevel, stone(x));
  const [fh0, fh1] = HAND.fingerH;
  const [c0, c1] = HAND.crease;
  const [u0, u1] = HAND.underKnuckle;
  const L = DECAL_LIFT;
  const b = HAND.fingerBevel;
  for (const [a0, a1, front, top, x, kx, kFront] of HAND.fingers) {
    k.bevelBox(a0, a1, HAND.fingerD0, front, fh0, fh1, b, stone(x));
    k.bevelBox(
      a0,
      a1,
      HAND.knuckleD0,
      kFront,
      HAND.knuckleH,
      top,
      HAND.knuckleBevel,
      stone(kx),
    );
    k.box(a0 + b, a1 - b, front, front + L, c0, c1, crack);
    k.box(a0 + b, a1 - b, front, front + L, u0, u1, crack);
  }
  for (const [a0, a1] of HAND.gaps) {
    k.box(a0, a1, HAND.fingerD0, HAND.gapFront, fh0, HAND.knuckleH, crack);
    k.box(
      a0,
      a1,
      HAND.knuckleD0,
      HAND.gapKnuckleFront,
      HAND.knuckleH,
      HAND.gapKnuckleTop,
      crack,
    );
  }
  for (const [a0, a1, d0, d1, h0, h1] of [...HAND.grooves, ...HAND.cracks])
    k.box(a0, a1, d0, d1, h0, h1, crack);
};

/** The rocket's red. */
const ROCKET_RED: Rgb = [0.8, 0.08, 0.05];

/** The rocket's white. */
const ROCKET_WHITE: Rgb = [0.95, 0.95, 0.92];

/** The rocket's plinth: a mid grey. */
const ROCKET_PLINTH: Rgb = [0.4, 0.4, 0.42];

/** The portholes' glass: a dark blue. */
const PORTHOLE_GLASS: Rgb = [0.1, 0.2, 0.35];

/**
 * The moon rocket's measures, in metres. The hull is an octagonal prism
 * of `sectors` faces whose apothem is `r` (so its corners are at
 * `r / cos(PI / 8)`, where the tail and the nose meet it), cut into
 * `bands` rings of panels from `h0` up to `h1`: square-ish panels about
 * 0.18 wide and 0.14 tall, red and white in a chequer. `plinth` is the
 * round plinth's top, which the three fins stand on.
 */
export const ROCKET = {
  sectors: 8,
  bands: 8,
  h0: 0.8,
  h1: 1.9,
  r: 0.22,
  plinth: 0.12,
} as const;

/** The hull's corner radius: where the tail cone ends and the nose starts. */
const CORNER = ROCKET.r / Math.cos(Math.PI / ROCKET.sectors);

/**
 * The rocket's parts other than the hull, in metres:
 * - `plinthR` and `plinthSides`: the round plinth's radius and facets;
 * - `tail` and `nose`: the lathe profiles `[radius, height]` of the white
 *   tail cone under the hull and the red nose over it, both `lathe`
 *   sides, so their corners meet the hull's;
 * - `fin`: one fin's outline `[a, h]` in its own frame, swept from the
 *   hull down to its foot on the plinth, `finHalf` thick either side of
 *   its plane; `finTurn` is the first fin's heading (its back), the other
 *   two follow a third of a turn apart;
 * - `ports`: the hull faces that carry a porthole (the four that face
 *   `+d`), each a disc of `portR` at `portH` standing `portOut` proud,
 *   with a rim ring of `rimR` and tube `rimTube`.
 */
const ROCKET_PARTS = {
  plinthR: 0.7,
  plinthSides: 16,
  lathe: 8,
  tail: [
    [0, 0.42],
    [0.12, 0.45],
    [0.2, 0.6],
    [CORNER, 0.8],
  ],
  nose: [
    [CORNER, 1.9],
    [0.18, 2.1],
    [0.1, 2.28],
    [0, 2.4],
  ],
  fin: [
    [0.15, 1.0],
    [0.62, 0.12],
    [0.45, 0.12],
    [0.15, 0.55],
  ],
  finHalf: 0.02,
  finTurn: -Math.PI / 2,
  ports: [6, 7, 0, 1],
  portR: 0.045,
  portH: 1.83,
  portOut: 0.012,
  rimR: 0.052,
  rimTube: 0.008,
} as const;

/**
 * The moon rocket: the plinth, the chequered hull built band by band and
 * face by face (face `i` in a frame turned by `(i + 0.5) * PI / 4`, so its
 * corners fall on the tail's and the nose's), the tail and the nose, the
 * three fins and the portholes.
 */
const moonRocket: HeroRecipe = ({ k, kitAt, s }) => {
  const red = s.tinted(ROCKET_RED);
  const white = s.tinted(ROCKET_WHITE);
  const P = ROCKET_PARTS;
  k.cylinder(
    0,
    0,
    0,
    ROCKET.plinth,
    P.plinthR,
    P.plinthSides,
    s.tinted(ROCKET_PLINTH),
  );
  const step = (2 * Math.PI) / ROCKET.sectors;
  const half = ROCKET.r * Math.tan(step / 2);
  const band = (ROCKET.h1 - ROCKET.h0) / ROCKET.bands;
  const faces = Array.from({ length: ROCKET.sectors }, (_, i) =>
    kitAt(yawed(ORIGIN, 0, 0, (i + 0.5) * step)),
  );
  for (let b = 0; b < ROCKET.bands; b++)
    faces.forEach((face, i) => {
      const h = ROCKET.h0 + b * band;
      face.panel(
        -half,
        half,
        ROCKET.r,
        h,
        h + band,
        (b + i) % 2 === 1 ? red : white,
        2 * half,
        band,
      );
    });
  k.lathe(0, 0, P.tail, P.lathe, white);
  k.lathe(0, 0, P.nose, P.lathe, red);
  for (let j = 0; j < 3; j++)
    kitAt(yawed(ORIGIN, 0, 0, P.finTurn + (j * 2 * Math.PI) / 3)).extrude(
      P.fin,
      -P.finHalf,
      P.finHalf,
      red,
    );
  const glass = s.tinted(PORTHOLE_GLASS);
  for (const i of P.ports) {
    const face = faces[i];
    if (face === undefined) continue;
    face.extrude(
      discOutline(0, P.portH, P.portR, 8),
      ROCKET.r,
      ROCKET.r + P.portOut,
      glass,
    );
    face.ring(
      0,
      ROCKET.r + P.portOut / 2,
      P.portH,
      P.rimR,
      P.rimTube,
      4,
      8,
      white,
      "inward",
    );
  }
};

/**
 * The hammer head's true size, in metres (the screen prop's): `long`
 * along its long axis (`a`), `side` across it and up.
 */
export const HAMMER_HEAD = { long: 0.235, side: 0.14 } as const;

/** The hammer head's grey metal. */
const HAMMER_GREY: Rgb = [0.45, 0.45, 0.47];

/** The knotwork bands: a slightly darker grey. */
const KNOT_GREY: Rgb = [0.36, 0.36, 0.38];

/**
 * The runes' ink: a grey darker than `KNOT_GREY`, so the runes read as
 * cut into the metal rather than painted on it (2.6f C13).
 */
export const RUNE_INK: Rgb = [0.28, 0.28, 0.3];

/** The handle's dark brown leather. */
const LEATHER: Rgb = [0.3, 0.18, 0.1];

/** The leather wraps: a darker brown. */
const WRAP: Rgb = [0.2, 0.12, 0.07];

/** The crack in the floor: near black. */
export const FLOOR_CRACK: Rgb = [0.15, 0.14, 0.14];

/** The dent under the head: a dark scorched grey. */
const FLOOR_DENT: Rgb = [0.28, 0.27, 0.26];

/**
 * The hammer's measures past its head, in metres and radians:
 * - `bevel`: the head's bevel;
 * - `bands`: the knotwork bands' centres along `a`, each `bandHalf` either
 *   side and `bandOut` proud of the head's sides and top; the panel
 *   between them, less a `bevel` margin all round, holds the runes;
 * - `handle`: the handle's centre `(c, h)` in the sideways frame (`c` runs
 *   towards `-d`), its angle up from the floor, length and square side;
 * - `wraps`: where along the handle (0 at its foot, 1 at its end) each
 *   leather wrap sits, `wrapLen` long and `wrapOut` thicker all round;
 * - `pommel`: its length and side, its centre `gap` past the handle's
 *   end, so it caps the end;
 * - `strap`: its angle and length, `width` its thickness in the handle's
 *   plane and `half` its half breadth along `a`, a flat ribbon whose broad
 *   side faces the front;
 * - `dent`: the flat dark patch under the head, a disc of radius `r`
 *   and `sides` facets, `h` over the floor: the head covers its middle,
 *   so it shows as a scorched ring where the hammer struck;
 * - `crack`: the floor's eight flat cracks, `h` over the floor (well over
 *   the dent, so neither fights the other or the floor). Each heads out
 *   from `from`, under the head, as a chain of bars given by `steps`
 *   (`[length, half width]`): wide at the root and narrowing to a point.
 *   At each joint the next bar turns by that crack's `kinks` entry and
 *   starts `overlap` back inside the last, so the crack jags without a
 *   notch; the bars share one colour and one height, so where two overlap
 *   nothing shows. `to` is the reach of the first crack's third bar end
 *   from the centre, `grow` times `k % 3` more for crack `k`. `forks`
 *   names the cracks that fork: a fork leaves where the second bar
 *   starts, `forkAt` along it, turned `forkTurn` away (to alternate
 *   sides), and narrows over `forkSteps`.
 */
export const HAMMER = {
  bevel: 0.015,
  bands: [-0.06, 0.06],
  bandHalf: 0.004,
  bandOut: 0.002,
  handle: {
    c: 0.1,
    h: 0.283,
    angle: (55 * Math.PI) / 180,
    length: 0.35,
    side: 0.035,
  },
  wraps: [0.3, 0.5, 0.7],
  wrapLen: 0.045,
  wrapOut: 0.006,
  pommel: { length: 0.04, side: 0.05, gap: 0.02 },
  strap: {
    angle: (-80 * Math.PI) / 180,
    length: 0.2,
    width: 0.005,
    half: 0.009,
  },
  dent: { r: 0.2, sides: 12, h: 0.002 },
  crack: {
    from: 0.05,
    h: 0.0045,
    steps: [
      [0.09, 0.013],
      [0.07, 0.008],
      [0, 0.004],
      [0.025, 0.0018],
    ],
    to: 0.27,
    grow: 0.03,
    overlap: 0.004,
    kinks: [
      [0.3, -0.35, 0.2],
      [-0.3, 0.25, -0.3],
      [0.15, -0.4, 0.25],
      [-0.2, 0.35, -0.15],
      [0.35, -0.2, 0.3],
      [-0.25, 0.3, -0.25],
      [0.2, -0.3, 0.35],
      [-0.35, 0.2, -0.2],
    ],
    forks: [1, 4, 6],
    forkAt: 0.01,
    forkTurn: 0.55,
    forkSteps: [
      [0.05, 0.0035],
      [0.025, 0.0015],
    ],
  },
} as const;

/**
 * The thunder hammer: the head with its two bands and its runes, the
 * handle with its wraps, the pommel and the strap in the sideways frame,
 * and the crack in the floor round the head.
 */
const thunderHammer: HeroRecipe = ({ k, kitAt, s }) => {
  const grey = s.tinted(HAMMER_GREY);
  const la = HAMMER_HEAD.long / 2;
  const ld = HAMMER_HEAD.side / 2;
  const top = HAMMER_HEAD.side;
  k.bevelBox(-la, la, -ld, ld, 0, top, HAMMER.bevel, grey);
  const knot = s.tinted(KNOT_GREY);
  const o = HAMMER.bandOut;
  for (const a of HAMMER.bands)
    k.box(
      a - HAMMER.bandHalf,
      a + HAMMER.bandHalf,
      -ld - o,
      ld + o,
      0,
      top + o,
      knot,
    );
  // The runes: `HAMMER_RUNES` fitted into the panel between the bands,
  // on `+d` and, through a kit turned half round, on `-d`, so each band
  // reads left to right from its own side.
  const ra0 = HAMMER.bands[0] + HAMMER.bandHalf + HAMMER.bevel;
  const ra1 = HAMMER.bands[1] - HAMMER.bandHalf - HAMMER.bevel;
  const r = fit(HAMMER_RUNES, ra0, ra1, HAMMER.bevel, top - HAMMER.bevel);
  const rune = s.tinted(RUNE_INK);
  for (const kit of [k, kitAt(yawed(ORIGIN, 0, 0, Math.PI))])
    pixelPanel(kit, HAMMER_RUNES, r.left, r.top, r.px, ld + DECAL_LIFT, (ch) =>
      ch === "#" ? rune : null,
    );
  // Sideways: `(c, h)` with `c` towards -d, and the thickness along `a`.
  const side = kitAt(sideways(ORIGIN));
  const H = HAMMER.handle;
  const dir = [Math.cos(H.angle), Math.sin(H.angle)] as const;
  const along = (t: number): [number, number] => [
    H.c + dir[0] * t,
    H.h + dir[1] * t,
  ];
  const leather = s.tinted(LEATHER);
  side.extrude(
    tiltedBar(H.c, H.h, H.angle, H.length, H.side),
    -H.side / 2,
    H.side / 2,
    leather,
  );
  const wrap = s.tinted(WRAP);
  const wide = H.side + 2 * HAMMER.wrapOut;
  for (const f of HAMMER.wraps) {
    const [c, h] = along((f - 0.5) * H.length);
    side.extrude(
      tiltedBar(c, h, H.angle, HAMMER.wrapLen, wide),
      -wide / 2,
      wide / 2,
      wrap,
    );
  }
  const Pm = HAMMER.pommel;
  const [pc, ph] = along(H.length / 2 + Pm.gap);
  side.extrude(
    tiltedBar(pc, ph, H.angle, Pm.length, Pm.side),
    -Pm.side / 2,
    Pm.side / 2,
    grey,
  );
  const St = HAMMER.strap;
  side.extrude(
    tiltedBar(
      pc + (Math.cos(St.angle) * St.length) / 2,
      ph + (Math.sin(St.angle) * St.length) / 2,
      St.angle,
      St.length,
      St.width,
    ),
    -St.half,
    St.half,
    leather,
  );
  const D = HAMMER.dent;
  k.cylinder(0, 0, 0, D.h, D.r, D.sides, s.tinted(FLOOR_DENT));
  const C = HAMMER.crack;
  const dark = s.tinted(FLOOR_CRACK);
  // A chain of bars from `start` along `f`, each turned by the next of
  // `turns` at its joint; returns the frame each bar was laid in.
  const chain = (
    f: Frame,
    start: number,
    steps: readonly (readonly [number, number])[],
    turns: readonly number[],
  ): Frame[] => {
    const frames: Frame[] = [];
    let at = f;
    let from = start;
    steps.forEach(([len, half], n) => {
      if (n > 0) at = yawed(at, from, 0, turns[n - 1] ?? 0);
      const a0 = n === 0 ? start : -C.overlap;
      const a1 = n === 0 ? start + len : len;
      kitAt(at).box(a0, a1, -half, half, 0, C.h, dark);
      frames.push(at);
      from = a1;
    });
    return frames;
  };
  const fixed = C.steps.reduce((sum, [len]) => sum + len, 0);
  for (let i = 0; i < 8; i++) {
    const theta = (i * Math.PI) / 4 + 0.2 * (((i * 7) % 3) - 1);
    const reach = C.to + C.grow * (i % 3);
    const steps = C.steps.map(([len, half], n) =>
      n === 2
        ? ([reach - C.from - fixed, half] as const)
        : ([len, half] as const),
    );
    const frames = chain(
      yawed(ORIGIN, 0, 0, theta),
      C.from,
      steps,
      C.kinks[i] ?? [],
    );
    const second = frames[1];
    if (second !== undefined && (C.forks as readonly number[]).includes(i)) {
      const side = i % 2 === 0 ? 1 : -1;
      chain(yawed(second, C.forkAt, 0, side * C.forkTurn), 0, C.forkSteps, [
        -side * 0.2,
      ]);
    }
  }
};

/** The exhibit kinds' recipes. */
export const EXHIBIT_RECIPES = {
  "stone-hand": stoneHand,
  "moon-rocket": moonRocket,
  "thunder-hammer": thunderHammer,
} satisfies Record<
  Extract<HeroKind, "stone-hand" | "moon-rocket" | "thunder-hammer">,
  HeroRecipe
>;
