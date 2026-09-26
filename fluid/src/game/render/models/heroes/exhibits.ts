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
 *   plinth, faceted slabs and dark cracks on its faces, four thick fingers
 *   curled on its front (`+d`) under big square knuckles, and the thumb
 *   folded across them from its `+a` side.
 * - The rocket is a slim red and white chequered hull (an octagonal prism
 *   of panels, `ROCKET`), a white tail cone and a red nose, standing on
 *   three swept fins on a low round plinth, one fin at its back and two
 *   spread at its front, with a row of portholes near the top facing `+d`.
 * - The hammer's grey bevelled head is the screen prop's true size
 *   (`HAMMER_HEAD`), its long axis along `a`, flat on the floor, with two
 *   faint knotwork bands. Its leather-wrapped handle rises from the head's
 *   top middle towards `-d` at 55 degrees and ends in a pommel with a
 *   strap hanging from it. Dark bars 4 mm high radiate from under the head
 *   as the floor's crack; the head is the hammer's one `bevelBox`.
 *
 * The numbers each kind is built to are named in a table above its
 * recipe: `HAND`, `ROCKET`, and `HAMMER_HEAD` with `HAMMER`.
 */

import type { HeroKind } from "../../../world/types";
import { DECAL_LIFT, frameAt, type Frame } from "../../kit";
import type { Rgb } from "../../looks";
import { discOutline, shade, sideways, tiltedBar, yawed } from "../common";
import { heroHalf, type HeroRecipe } from "./common";

/** The recipe's own frame: the origin, facing north. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/** The hand's plinth: a dark stone grey. */
const HAND_PLINTH: Rgb = [0.3, 0.3, 0.32];

/** The hand's brick-red stone. */
const STONE_RED: Rgb = [0.55, 0.16, 0.12];

/** The dark red-brown in the hand's cracks. */
const STONE_CRACK: Rgb = [0.3, 0.1, 0.08];

/**
 * The stone hand's measures, in metres, `[lo, hi]` pairs along `a`, `d`
 * and `h`:
 * - `plinth`: the plinth's height and bevel (it fills the footprint);
 * - `base`: the broken edge, five irregular slabs round the forearm's
 *   foot, each `[a0, a1, d0, d1, h1]` from the plinth's top up to `h1`;
 * - `forearm`, `wrist` and `fist`: bevelled blocks `[a, d, h, bevel]`,
 *   the fist wider than the forearm, so the hand reads as the big end;
 * - `slabs`: the faceting slabs on the forearm's front and back;
 * - `fingers`: each finger's `a` range (index at `+a`, little finger at
 *   `-a`) and its knuckle's top; every finger is a block over `fingerD`
 *   and `fingerH` bevelled by `fingerBevel`, so the gaps between them
 *   read, with a dark crease across its front over `crease` (the joint
 *   of the curled finger), under a big square knuckle over `knuckleD`
 *   from `knuckleH`, bevelled by `knuckleBevel` and standing a little
 *   proud of the fingers at the front;
 * - `thumb`, `thumbTip` and `thumbRoot`: bevelled blocks, the thumb
 *   folded across the fingers from the `+a` side, its tip tucked in at
 *   `-a`, and the ball of the thumb joining it to the fist;
 * - `cracks`: dark bars laid `DECAL_LIFT` proud on the flat faces.
 * Everything fits inside the 0.45 half width and under the 1.4 top.
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
  forearm: [[-0.2, 0.2], [-0.17, 0.17], [0.3, 0.85], 0.04],
  slabs: [
    [[-0.16, 0.16], [0.17, 0.2], [0.45, 0.8], 0.02],
    [[-0.13, 0.13], [-0.2, -0.17], [0.5, 0.78], 0.02],
  ],
  wrist: [[-0.17, 0.17], [-0.15, 0.15], [0.85, 0.95], 0.03],
  fist: [[-0.25, 0.25], [-0.15, 0.12], [0.95, 1.3], 0.04],
  fingers: [
    [0.127, 0.245, 1.4],
    [0.003, 0.121, 1.4],
    [-0.121, -0.003, 1.385],
    [-0.245, -0.127, 1.36],
  ],
  fingerD: [0.12, 0.21],
  fingerH: [1.0, 1.28],
  fingerBevel: 0.015,
  knuckleD: [0.02, 0.22],
  knuckleH: 1.26,
  knuckleBevel: 0.025,
  crease: [1.16, 1.172],
  thumb: [[-0.1, 0.25], [0.21, 0.29], [1.0, 1.13], 0.02],
  thumbTip: [[-0.16, -0.09], [0.19, 0.27], [1.0, 1.11], 0.02],
  thumbRoot: [[0.16, 0.27], [0.08, 0.26], [0.95, 1.13], 0.03],
  cracks: [
    // The forearm's +a side.
    ["a+", [-0.056, -0.044], [0.42, 0.72]],
    ["a+", [-0.044, 0.12], [0.6, 0.612]],
    // The forearm's -a side.
    ["a-", [0.02, 0.032], [0.36, 0.8]],
    ["a-", [-0.12, 0.02], [0.52, 0.532]],
    // The front slab.
    ["front", [0.03, 0.042], [0.48, 0.78]],
    ["front", [-0.13, 0.03], [0.64, 0.652]],
    // The fist's back and its -a side.
    ["fistBack", [-0.02, -0.008], [0.99, 1.26]],
    ["fistSide", [-0.1, 0.08], [1.12, 1.132]],
  ],
} as const;

/** A bevelled block of `HAND`: its `a`, `d` and `h` ranges and its bevel. */
type Block = readonly [
  readonly [number, number],
  readonly [number, number],
  readonly [number, number],
  number,
];

/**
 * The stone hand: plinth, broken base, forearm with its slabs, wrist,
 * fist, fingers and knuckles, the folded thumb, and the cracks. The slabs
 * and the base are a shade darker than the forearm, so the facets read
 * even under flat light.
 */
const stoneHand: HeroRecipe = ({ k, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const red = s.tinted(STONE_RED);
  const facet = s.tinted(shade(STONE_RED, 0.88));
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
  for (const [a0, a1, d0, d1, h1] of HAND.base)
    k.box(a0, a1, d0, d1, HAND.plinth.h, h1, facet);
  const block = ([[a0, a1], [d0, d1], [h0, h1], bevel]: Block, sf = red) =>
    k.bevelBox(a0, a1, d0, d1, h0, h1, bevel, sf);
  block(HAND.forearm);
  for (const slab of HAND.slabs) block(slab, facet);
  block(HAND.wrist);
  block(HAND.fist);
  const [fd0, fd1] = HAND.fingerD;
  const [fh0, fh1] = HAND.fingerH;
  const [kd0, kd1] = HAND.knuckleD;
  const [c0, c1] = HAND.crease;
  const L = DECAL_LIFT;
  for (const [a0, a1, top] of HAND.fingers) {
    k.bevelBox(a0, a1, fd0, fd1, fh0, fh1, HAND.fingerBevel, red);
    k.bevelBox(a0, a1, kd0, kd1, HAND.knuckleH, top, HAND.knuckleBevel, facet);
    const b = HAND.fingerBevel;
    k.box(a0 + b, a1 - b, fd1, fd1 + L, c0, c1, crack);
  }
  for (const part of [HAND.thumbRoot, HAND.thumb, HAND.thumbTip]) block(part);
  const fa1 = HAND.forearm[0][1];
  const slabFront = HAND.slabs[0][1][1];
  const fistBack = HAND.fist[1][0];
  const fistSide = HAND.fist[0][0];
  for (const [face, [u0, u1], [h0, h1]] of HAND.cracks) {
    if (face === "a+") k.box(fa1, fa1 + L, u0, u1, h0, h1, crack);
    else if (face === "a-") k.box(-fa1 - L, -fa1, u0, u1, h0, h1, crack);
    else if (face === "front")
      k.box(u0, u1, slabFront, slabFront + L, h0, h1, crack);
    else if (face === "fistBack")
      k.box(u0, u1, fistBack - L, fistBack, h0, h1, crack);
    else k.box(fistSide - L, fistSide, u0, u1, h0, h1, crack);
  }
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

/** The handle's dark brown leather. */
const LEATHER: Rgb = [0.3, 0.18, 0.1];

/** The leather wraps: a darker brown. */
const WRAP: Rgb = [0.2, 0.12, 0.07];

/** The crack in the floor: near black. */
const FLOOR_CRACK: Rgb = [0.12, 0.12, 0.13];

/**
 * The hammer's measures past its head, in metres and radians:
 * - `bevel`: the head's bevel;
 * - `bands`: the knotwork bands' centres along `a`, each `bandHalf` either
 *   side and `bandOut` proud of the head's sides and top;
 * - `handle`: the handle's centre `(c, h)` in the sideways frame (`c` runs
 *   towards `-d`), its angle up from the floor, length and square side;
 * - `wraps`: where along the handle (0 at its foot, 1 at its end) each
 *   leather wrap sits, `wrapLen` long and `wrapOut` thicker all round;
 * - `pommel`: its length and side, its centre `gap` past the handle's
 *   end, so it caps the end;
 * - `strap`: its angle and length, `width` its thickness in the handle's
 *   plane and `half` its half breadth along `a`, a flat ribbon whose broad
 *   side faces the front;
 * - `crack`: the floor's eight cracks, each heading out from under the
 *   head: an inner bar from `from` to `bend`, `half` wide either side and
 *   `h` high, then a thinner outer bar (`outHalf`, `outH`) kinked by
 *   `kink` times -1, 0 or 1 at the bend and running on to `to` plus
 *   `grow` times `k % 3` from the centre, so each crack narrows and
 *   jags; `branch` names the cracks that fork, `branchAt` along the inner
 *   bar, at `branchTurn`, each branch `branchLen` long, `branchHalf`
 *   wide and `branchH` high. Every bar is a little lower than the one it
 *   leaves, so no two tops share a plane.
 */
const HAMMER = {
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
  crack: {
    from: 0.06,
    bend: 0.16,
    to: 0.25,
    grow: 0.03,
    half: 0.007,
    h: 0.004,
    outHalf: 0.004,
    outH: 0.003,
    kink: 0.3,
    branch: [1, 4, 6],
    branchAt: 0.14,
    branchTurn: Math.PI / 6,
    branchLen: 0.07,
    branchHalf: 0.003,
    branchH: 0.0025,
  },
} as const;

/**
 * The thunder hammer: the head with its two bands, the handle with its
 * wraps, the pommel and the strap in the sideways frame, and the crack in
 * the floor round the head.
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
  const C = HAMMER.crack;
  const dark = s.tinted(FLOOR_CRACK);
  for (let i = 0; i < 8; i++) {
    const theta = (i * Math.PI) / 4 + 0.2 * (((i * 7) % 3) - 1);
    const f = yawed(ORIGIN, 0, 0, theta);
    kitAt(f).box(C.from, C.bend, -C.half, C.half, 0, C.h, dark);
    const kink = C.kink * (((i * 5) % 3) - 1);
    kitAt(yawed(f, C.bend, 0, kink)).box(
      0,
      C.to + C.grow * (i % 3) - C.bend,
      -C.outHalf,
      C.outHalf,
      0,
      C.outH,
      dark,
    );
    if ((C.branch as readonly number[]).includes(i))
      kitAt(yawed(f, C.branchAt, 0, C.branchTurn)).box(
        0,
        C.branchLen,
        -C.branchHalf,
        C.branchHalf,
        0,
        C.branchH,
        dark,
      );
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
