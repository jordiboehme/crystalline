/**
 * The other rare props (2.6d): the ooze canister cluster, the saucer
 * poster and the designer tower.
 *
 * - **Ooze canisters.** Squat silver-grey canisters, 0.28 m across and
 *   0.4 m tall, each a ten-sided lathe with a dark band near its top and
 *   its bottom, a dark screw lid and a ridged cap. An irregular crack in
 *   the side of each standing one shows glowing green ooze inside a dark
 *   torn edge, a thin runnel of it seeping down from the crack's foot, and
 *   the maker's letters (`MARKS.canister`) are printed dark on the facet
 *   beside it. Crack and letters lie flat on facets of the lathe, whose
 *   angles are read from its first vertex, on `+d`. Variant 0 stands two
 *   canisters and lays a third on its side, variant 1 one of each: the
 *   standing ones at the front, their cracks and letters to the room, the
 *   tipped one behind them, lying along `a` on its bands, lid off, its open
 *   mouth at `+a` showing a glowing disc inside a dark rim. From the mouth a
 *   puddle spreads across the floor and curls round to the front (C14):
 *   overlapping flat discs of differing facet counts, each a dark stain
 *   with a smaller glowing film on it, every disc half a millimetre higher
 *   than the one before so no two faces are coplanar. The puddle is part
 *   of the cluster's mesh and inside its footprint, which collides, so
 *   nobody wades into it. The bank is `breathe` (`PROP_BANK`): cracks,
 *   runnels, mouth and films breathe together in group 0, and their tint
 *   is set above 1 so the glow's low phase still shows a clear green,
 *   never a dark one.
 * - **Saucer poster.** A portrait paper sheet, 0.6 by 0.9 m, on its wall
 *   from 1.1 to 2.0 m. Its picture (`SAUCER_PICTURE`) is a grainy,
 *   washed-out photograph: a grey flying saucer over a ragged dark line
 *   of trees under a pale grey-blue sky, a few warm points of light under
 *   the disc, the sky dithered between two close greys and the panel
 *   layer's fine grain laid over the whole picture once (`PHOTO_UV`). On
 *   the dark band under it the caption (`MARKS.poster`) runs in tall,
 *   condensed white capitals across most of the width, so it reads from
 *   across a room. Four pins hold the sheet; the lower right corner has
 *   come loose and curls a little off the wall, a white flap hinged on the
 *   sheet, with its pin above it, and the sheet and the band are cut back
 *   under it. No light.
 * - **Designer tower.** An off-white PC tower, 0.25 by 0.43 by 0.62 m: an
 *   ordinary box behind, and in front a flowing curved facade, drawn as a
 *   smooth curve, that bulges most low down and sweeps back towards the
 *   top. The drive slots and the floppy slot are set into the curve, each
 *   just proud of it at its own height. Below them a round power button in
 *   a dark collar with a green power light and an amber drive light beside
 *   it, and lower still a small dark display showing the clock speed
 *   (`MARKS.towerClock`) in glowing red seven-segment digits (the font's
 *   own digits) next to a turbo button. Near the top a small badge plate
 *   carries the maker's name (`MARKS.towerBadge`) with the designer's
 *   signature squiggle (`SIGNATURE`) under it. The lights and the digits
 *   are steady `s.signal` lights; the bank is `steady`.
 *
 * Every string, and every picture that is a logo or a badge, comes from
 * `../marks.ts` (C16); the saucer's photograph (`SAUCER_PICTURE`) is
 * defined here instead, since it is the original's image, not a logo or
 * a text mark. The small marks (the canister's letters, the tower's
 * digits and badge) are `pixelBoxes` standing `MARK_PROUD` proud of their
 * faces; the poster's picture and caption are quads at `DECAL_LIFT`, one
 * per run of the font or the picture.
 */

import type { PropKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { DECAL_LIFT, frameAt, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import {
  discOutline,
  profileAlong,
  sideways,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import {
  MARK_PROUD,
  pixelBoxes,
  pixelRuns,
  runsOf,
  textRows,
} from "../heroes/pixels";
import { MARKS, SIGNATURE } from "../marks";
import type { PropRecipe } from "./common";

/** The frame every recipe here builds in. */
const ORIGIN = frameAt([0, 0, 0], 0);

// ---------------------------------------------------------------------------
// The ooze canisters.

/** The canisters' silver-grey metal. */
const CANISTER_METAL: Rgb = [0.72, 0.74, 0.76];

/** The canisters' bands, lids and rims: near black. */
export const LID_DARK: Rgb = [0.2, 0.2, 0.22];

/** The maker's letters printed on each standing canister. */
export const LABEL_DARK: Rgb = [0.08, 0.08, 0.08];

/**
 * The ooze's glowing green, in blink group 0 of the breathe bank. Above 1
 * so that at the glow's low phase (0.4 of full) it is still a clear green,
 * and at the peak it clips to a pale, bright one.
 */
const OOZE: Rgb = [0.5, 1.8, 0.3];

/** The puddle's stain under each glowing film: a dark, wet green. */
const STAIN: Rgb = [0.1, 0.25, 0.08];

/** The canisters' facet count, the lathe's and the tipped body's. */
const CANISTER_SIDES = 10;

/** The canister's radius and body height, in metres; the lid and cap stand on the body. */
const CANISTER_R = 0.14;
const BODY_TOP = 0.36;

/**
 * A standing canister's body, `[r, h]`: out across the bottom, a small
 * chamfer, up the side and a chamfer in at the top.
 */
const BODY_PROFILE: readonly (readonly [number, number])[] = [
  [0, 0],
  [0.13, 0],
  [CANISTER_R, 0.02],
  [CANISTER_R, 0.34],
  [0.13, BODY_TOP],
  [0, BODY_TOP],
];

/** The ridged cap on the lid, `[r, h]`: two grooves under a flat top at 0.42 m. */
const CAP_PROFILE: readonly (readonly [number, number])[] = [
  [0, 0.4],
  [0.055, 0.4],
  [0.055, 0.406],
  [0.049, 0.409],
  [0.055, 0.412],
  [0.049, 0.415],
  [0.052, 0.42],
  [0, 0.42],
];

/** The bands: their heights on a standing canister and their tube radius. */
const BAND_HS = [0.035, 0.325] as const;
const BAND_TUBE = 0.012;

/** Half the angle one facet of the lathe spans; facet `i` is centred at `(2i + 1)` times it from `+d`. */
const HALF_FACET = Math.PI / CANISTER_SIDES;

/** A facet's distance from the canister's axis, in metres. */
const APOTHEM = CANISTER_R * Math.cos(HALF_FACET);

/**
 * The crack, `[a, h]` on its facet: a ragged split about 0.2 m tall,
 * widest in its lower middle, narrowing to hairlines at both ends, with a
 * short branch torn off to the right.
 */
const CRACK: readonly (readonly [number, number])[] = [
  [0.001, 0.105],
  [0.006, 0.13],
  [0.003, 0.15],
  [0.011, 0.175],
  [0.008, 0.2],
  [0.02, 0.215],
  [0.026, 0.232],
  [0.012, 0.225],
  [0.01, 0.245],
  [0.004, 0.27],
  [0.007, 0.29],
  [0.0, 0.305],
  [-0.004, 0.285],
  [-0.003, 0.26],
  [-0.012, 0.24],
  [-0.008, 0.215],
  [-0.014, 0.19],
  [-0.006, 0.165],
  [-0.009, 0.14],
  [-0.003, 0.12],
];

/** The crack's torn dark edge: the crack's outline grown this much across and along. */
const CRACK_EDGE = { a: 1.3, h: 1.06 } as const;

/** How much wider than `CRACK` the crack is drawn, so more ooze shows. */
const CRACK_WIDEN = 1.25;

/**
 * A thin runnel of ooze seeping from the crack's foot down towards the
 * lower band, `[a, h]` on the same facet.
 */
const DRIP: readonly (readonly [number, number])[] = [
  [-0.001, 0.125],
  [-0.004, 0.1],
  [-0.002, 0.08],
  [-0.004, 0.062],
  [-0.008, 0.062],
  [-0.007, 0.08],
  [-0.009, 0.1],
  [-0.005, 0.125],
];

/** The letters' pixel size (15 columns, 0.075 m) and their top, in metres. */
const LABEL_PX = 0.005;
const LABEL_TOP = 0.215;

/** The tipped canister's axis height: it rests on its bands. */
const TIPPED_H = CANISTER_R + BAND_TUBE;

/** The tipped canister's open mouth: the rim's ring radius and the glowing disc's. */
const RIM_R = 0.128;
const MOUTH_R = 0.12;

/**
 * The puddle: each disc's centre along `a` past the mouth and across `d`
 * from the tipped canister's axis, its stain's radius and its facet
 * count, which differs from disc to disc so the outline is no chain of
 * circles. It runs out of the mouth and curls round towards the front,
 * `+d`, past the standing canisters. The films are `FILM_INSET` smaller.
 */
const PUDDLE: readonly (readonly [
  da: number,
  dd: number,
  r: number,
  sides: number,
])[] = [
  [0.08, 0.01, 0.1, 9],
  [0.16, 0.13, 0.12, 11],
  [0.12, 0.28, 0.1, 8],
  [0.21, 0.38, 0.07, 7],
  [0.06, 0.4, 0.06, 7],
];
const STAIN_THICK = 0.002;
const FILM_THICK = 0.0015;
const FILM_INSET = 0.02;
const PUDDLE_STEP = 0.0005;

/**
 * Each variant's layout: the standing canisters' centres, and the tipped
 * one's bottom, mouth and axis `d` and the scale of its puddle, which
 * variant 1's smaller footprint shrinks. The standing ones stand at the
 * front, their cracks and letters to the room, and the tipped one lies
 * behind them with its mouth to the right, where the puddle runs out
 * past them.
 */
const CLUSTERS: readonly {
  standing: readonly (readonly [number, number])[];
  tipped: { a0: number; a1: number; d: number; puddle: number };
}[] = [
  {
    standing: [
      [-0.33, 0.12],
      [-0.04, 0.22],
    ],
    tipped: { a0: -0.16, a1: 0.2, d: -0.2, puddle: 1 },
  },
  {
    standing: [[-0.22, 0.15]],
    tipped: { a0: -0.3, a1: 0.06, d: -0.18, puddle: 0.75 },
  },
];

/** A frame at `(a, d)` turned so its `inward` points at lathe angle `angle` from `+d`. */
const facing = (a: number, d: number, angle: number) =>
  yawed(ORIGIN, a, d, -angle);

/** One standing canister at `(a, d)`: body, bands, lid, cap, crack and letters. */
function standingCanister(
  kitAt: KitAt,
  k: Kit,
  s: Surfaces,
  a: number,
  d: number,
): void {
  const dark = s.tinted(LID_DARK);
  k.lathe(a, d, BODY_PROFILE, CANISTER_SIDES, s.tinted(CANISTER_METAL));
  for (const h of BAND_HS)
    k.ring(a, d, h, CANISTER_R, BAND_TUBE, 4, CANISTER_SIDES, dark, "up");
  k.cylinder(a, d, BODY_TOP, 0.4, 0.07, CANISTER_SIDES, dark);
  k.lathe(a, d, CAP_PROFILE, CANISTER_SIDES, dark);
  // The crack on the facet just right of `+d`, the letters on the one just
  // left of it: both face the room when the cluster does.
  const crack = kitAt(facing(a, d, HALF_FACET));
  const split = CRACK.map(([ca, ch]) => [ca * CRACK_WIDEN, ch] as const);
  crack.extrude(
    split.map(([ca, ch]) => [
      ca * CRACK_EDGE.a,
      0.2 + (ch - 0.2) * CRACK_EDGE.h,
    ]),
    APOTHEM - 0.001,
    APOTHEM + 0.001,
    dark,
  );
  const ooze = s.blink(OOZE, 0);
  crack.extrude(split, APOTHEM - 0.002, APOTHEM + 0.002, ooze);
  crack.extrude(DRIP, APOTHEM - 0.001, APOTHEM + 0.0015, ooze);
  const rows = textRows(MARKS.canister);
  const w = (rows[0]?.length ?? 0) * LABEL_PX;
  const ink = s.tinted(LABEL_DARK);
  pixelBoxes(
    kitAt(facing(a, d, -HALF_FACET)),
    rows,
    -w / 2,
    LABEL_TOP,
    LABEL_PX,
    APOTHEM,
    APOTHEM + MARK_PROUD,
    (ch) => (ch === "#" ? ink : null),
  );
}

/**
 * The tipped canister from `a0` (its bottom) to `a1` (its open mouth) at
 * depth `d`, lying on its bands, and the puddle spreading from its mouth,
 * scaled by `puddle`.
 */
function tippedCanister(
  kitAt: KitAt,
  k: Kit,
  s: Surfaces,
  { a0, a1, d, puddle }: { a0: number; a1: number; d: number; puddle: number },
): void {
  const dark = s.tinted(LID_DARK);
  const ooze = s.blink(OOZE, 0);
  k.cylinderAlong(
    a0,
    a1,
    d,
    TIPPED_H,
    CANISTER_R,
    CANISTER_SIDES,
    s.tinted(CANISTER_METAL),
  );
  // In the sideways frame `d'` runs along `a` and `a'` against `d`, so a
  // ring standing in its wall plane rings the canister's axis.
  const side = kitAt(sideways(ORIGIN));
  for (const h of BAND_HS)
    side.ring(
      -d,
      a0 + h,
      TIPPED_H,
      CANISTER_R,
      BAND_TUBE,
      4,
      CANISTER_SIDES,
      dark,
      "inward",
    );
  side.ring(-d, a1, TIPPED_H, RIM_R, BAND_TUBE, 4, 12, dark, "inward");
  side.extrude(discOutline(-d, TIPPED_H, MOUTH_R, 12), a1, a1 + 0.002, ooze);
  // The puddle: stain and film per disc, each disc a step higher, each a
  // lathe with no bottom, since the floor and the stain hide it.
  const stain = s.tinted(STAIN);
  PUDDLE.forEach(([da, dd, r, sides], i) => {
    const top = STAIN_THICK + i * PUDDLE_STEP;
    const film = top + FILM_THICK;
    const [pa, pd, pr] = [a1 + da * puddle, d + dd * puddle, r * puddle];
    const fr = pr - FILM_INSET;
    k.lathe(
      pa,
      pd,
      [
        [pr, 0],
        [pr, top],
        [0, top],
      ],
      sides,
      stain,
    );
    k.lathe(
      pa,
      pd,
      [
        [fr, top],
        [fr, film],
        [0, film],
      ],
      sides,
      ooze,
    );
  });
}

/** The canister cluster: the variant's standing canisters and its tipped one. */
const oozeCanisters: PropRecipe = ({ k, kitAt, s, variant }) => {
  const cluster = CLUSTERS[variant];
  if (!cluster)
    throw new Error(`ooze-canisters: no variant ${String(variant)}`);
  for (const [a, d] of cluster.standing) standingCanister(kitAt, k, s, a, d);
  tippedCanister(kitAt, k, s, cluster.tipped);
};

// ---------------------------------------------------------------------------
// The saucer poster.

/**
 * The poster's picture, 26 rows of 24, row 0 at the top: `s` sky and `h`
 * haze, dithered for the photograph's grain and hazier towards the
 * horizon; `u` the saucer's lit top and `d` its dark underside; `g` the
 * warm points of light just under the disc; `t` the dark line of trees,
 * ragged from row 19 and solid along the bottom.
 */
export const SAUCER_PICTURE: readonly string[] = [
  "hssssssssssshssssshshshs",
  "ssssssshssssssssssssssss",
  "sssshssshsshssssshssssss",
  "ssssshsssssssshsssssshss",
  "shssshssssssshsshshsshhs",
  "sshshsshhhsshssssssssshs",
  "ssssssssssssshssshssssss",
  "hsssshsssssuuhhhsshshsss",
  "hssssshssuuuuuuhssshhsss",
  "sssshuuuuuuuuuuuuuushhhs",
  "ssssddddddddddddddddhsss",
  "hssshssddddddddddhssssss",
  "hshshhssgshggshgssshhsss",
  "sshhshsssshgsshhhsshshhh",
  "shhsshshshhsshhhsshhshhs",
  "hhhsshhsshshhhsshsshhhss",
  "hhsshhssshhshshshhssssss",
  "shhshhhhshshhhhhshhhhhhh",
  "shshsshhhshsshhsssshshsh",
  "sstssssssttsssssstssstss",
  "stttssssttttsssstttsttts",
  "tttttssttttttssttttttttt",
  "ttttttstttttttsttttttttt",
  "ttttttttttttttsttttttttt",
  "tttttttttttttttttttttttt",
  "tttttttttttttttttttttttt",
];

/** Each picture character's colour. */
const PICTURE_TINTS: Readonly<Record<string, Rgb>> = {
  s: [0.72, 0.76, 0.8],
  h: [0.8, 0.82, 0.84],
  u: [0.64, 0.65, 0.67],
  d: [0.3, 0.3, 0.32],
  g: [0.95, 0.9, 0.7],
  t: [0.12, 0.14, 0.12],
};

/** The poster's paper. */
const PAPER_WHITE: Rgb = [0.93, 0.93, 0.9];

/** The dark band under the picture. */
const BAND_DARK: Rgb = [0.07, 0.07, 0.08];

/** The caption's white. */
export const CAPTION_WHITE: Rgb = [0.97, 0.97, 0.97];

/** The pins' metal. */
export const PIN_METAL: Rgb = [0.75, 0.75, 0.78];

/** The sheet's half width, its thickness and its bottom and top edges, in metres (C3, C11). */
const POSTER_HALF = 0.3;
const SHEET_D = 0.004;
const POSTER_H = [1.1, 2.0] as const;

/** The picture's pixel size (24 columns fill the 0.6 m width) and its bottom edge. */
const PICTURE_PX = 0.025;
const PICTURE_BOTTOM = POSTER_H[1] - SAUCER_PICTURE.length * PICTURE_PX;

/** The band's face, a little proud of the sheet, and the box the caption fills. */
const BAND_FACE = SHEET_D + 0.002;
const CAPTION_BOX = [-0.28, 0.28, 1.17, 1.28] as const;

/**
 * How many times taller than wide the caption's pixels are: the caption
 * fills the width of `CAPTION_BOX` and stands 7 cm tall in its middle,
 * condensed capitals that read from across a room.
 */
const CAPTION_STRETCH = 1.68;

/** The pins: their side, their inset from the sheet's edges and how far they stand out. */
const PIN = 0.008;
const PIN_INSET = 0.015;
const PIN_FRONT = SHEET_D + DECAL_LIFT + 0.006;

/** The curled corner: the flap's legs, its thickness and how far it is turned off the wall. */
const CURL = 0.07;
const CURL_THICK = 0.002;
const CURL_YAW = (25 * Math.PI) / 180;

/**
 * The picture's texture coordinates: the whole picture maps into one
 * quarter of the panel layer, from `PHOTO_UV[0]` to `PHOTO_UV[1]` both
 * ways, clear of the layer's bevel lines (at every half) and of every
 * whole uv line. So the layer's fine grain lies over the photograph once,
 * as the print's grain, and a look that draws edge lines along whole uv
 * lines leaves the picture alone instead of outlining every run of it.
 */
const PHOTO_UV = [0.02, 0.48] as const;

/**
 * The picture at depth `d`: one quad per run of `SAUCER_PICTURE`, as
 * `pixelPanel` lays them, each quad taking its own share of the picture's
 * uv square (`PHOTO_UV`).
 */
function photo(k: Kit, s: Surfaces, d: number): void {
  const rows = SAUCER_PICTURE.length;
  const cols = SAUCER_PICTURE[0]?.length ?? 0;
  const [u0, u1] = PHOTO_UV;
  const span = u1 - u0;
  for (const r of runsOf(SAUCER_PICTURE)) {
    const tint = PICTURE_TINTS[r.ch];
    if (tint === undefined) throw new Error(`photo: no colour ${r.ch}`);
    k.panel(
      -POSTER_HALF + r.col * PICTURE_PX,
      -POSTER_HALF + (r.col + r.len) * PICTURE_PX,
      d,
      POSTER_H[1] - (r.row + 1) * PICTURE_PX,
      POSTER_H[1] - r.row * PICTURE_PX,
      s.tinted(tint),
      (r.len / cols) * span,
      span / rows,
      u0 + (r.col / cols) * span,
      u0 + ((rows - 1 - r.row) / rows) * span,
    );
  }
}

/** The saucer poster: sheet, picture, band, caption, pins and the curled corner. */
const saucerPoster: PropRecipe = ({ k, kitAt, s }) => {
  const [h0, h1] = POSTER_H;
  const paper = s.tinted(PAPER_WHITE);
  // The sheet and the band, both cut back under the curled corner.
  const cut = (top: number): [number, number][] => [
    [-POSTER_HALF, h0],
    [POSTER_HALF - CURL, h0],
    [POSTER_HALF, h0 + CURL],
    [POSTER_HALF, top],
    [-POSTER_HALF, top],
  ];
  k.extrude(cut(h1), 0, SHEET_D, paper);
  k.extrude(cut(PICTURE_BOTTOM), SHEET_D, BAND_FACE, s.tinted(BAND_DARK));
  photo(k, s, SHEET_D + DECAL_LIFT);
  // The caption: the font's runs across the width, each pixel
  // `CAPTION_STRETCH` times as tall as it is wide, as the original's
  // condensed capitals are.
  const caption = textRows(MARKS.poster);
  const [ca0, ca1, ch0, ch1] = CAPTION_BOX;
  const cpx = (ca1 - ca0) / (caption[0]?.length ?? 1);
  const cph = cpx * CAPTION_STRETCH;
  const ctop = (ch0 + ch1) / 2 + (caption.length * cph) / 2;
  const white = s.tinted(CAPTION_WHITE);
  for (const r of pixelRuns(caption))
    k.panel(
      ca0 + r.col * cpx,
      ca0 + (r.col + r.len) * cpx,
      BAND_FACE + DECAL_LIFT,
      ctop - (r.row + 1) * cph,
      ctop - r.row * cph,
      white,
    );
  // The pins: three at their corners, the lower right one above the flap
  // that has come loose under it.
  const metal = s.tinted(PIN_METAL);
  const pinAt = (a: number, h: number) => {
    k.box(
      a - PIN / 2,
      a + PIN / 2,
      SHEET_D,
      PIN_FRONT,
      h - PIN / 2,
      h + PIN / 2,
      metal,
    );
  };
  const ea = POSTER_HALF - PIN_INSET;
  pinAt(-ea, h1 - PIN_INSET);
  pinAt(ea, h1 - PIN_INSET);
  pinAt(-ea, h0 + PIN_INSET);
  pinAt(ea, h0 + CURL + 0.02);
  // The flap, hinged on the sheet at its inner end and turned off the wall.
  kitAt(yawed(ORIGIN, POSTER_HALF - CURL, SHEET_D, CURL_YAW)).extrude(
    [
      [0, h0],
      [CURL, h0],
      [CURL, h0 + CURL],
    ],
    0,
    CURL_THICK,
    paper,
  );
};

// ---------------------------------------------------------------------------
// The designer tower.

/** The tower's off-white plastic. */
const TOWER_WHITE: Rgb = [0.92, 0.91, 0.87];

/** The slots, the display window and the button collars: near black. */
const SLOT_DARK: Rgb = [0.1, 0.1, 0.11];

/** The buttons' plastic: a mid grey that stands out from the case. */
const BUTTON_GREY: Rgb = [0.62, 0.62, 0.6];

/** The display's red digits, a steady signal light. */
export const CLOCK_RED: Rgb = [1.0, 0.15, 0.1];

/** The power and drive lights. */
const POWER_GREEN: Rgb = [0.2, 1.0, 0.3];
const DRIVE_AMBER: Rgb = [1.0, 0.6, 0.1];

/** The badge's letters and signature. */
export const BADGE_GREY: Rgb = [0.4, 0.4, 0.42];

/** The badge plate: a silvery grey. */
const BADGE_PLATE: Rgb = [0.84, 0.84, 0.86];

/** The box behind the facade, in metres. */
const TOWER = { a: 0.125, back: -0.215, front: 0.13, top: 0.62 } as const;

/**
 * The facade's side profile, `[d, h]`, bottom to top: out from the box's
 * front, bulging most low down and sweeping back to it at the top.
 */
const FACADE: readonly (readonly [number, number])[] = [
  [0.13, 0],
  [0.19, 0.03],
  [0.215, 0.14],
  [0.205, 0.32],
  [0.18, 0.46],
  [0.15, 0.57],
  [0.13, 0.62],
];

/** How many pieces each span of `FACADE` is cut into for the smooth curve. */
const FACADE_STEPS = 2;

/**
 * The facade's profile drawn smooth: a Catmull-Rom curve through every
 * point of `FACADE`, each span cut into `FACADE_STEPS`, so the front
 * reads as one flowing curve rather than six flat facets. It passes
 * through the points themselves, so the bulge keeps its depth.
 */
const SMOOTH_FACADE: readonly (readonly [number, number])[] = (() => {
  const at = (i: number) =>
    FACADE[Math.max(0, Math.min(FACADE.length - 1, i))] ?? [0, 0];
  const out: [number, number][] = [];
  for (let i = 0; i + 1 < FACADE.length; i++) {
    const [p0, p1, p2, p3] = [at(i - 1), at(i), at(i + 1), at(i + 2)];
    for (let j = 0; j < FACADE_STEPS; j++) {
      const t = j / FACADE_STEPS;
      const c = (k: 0 | 1) =>
        0.5 *
        (2 * p1[k] +
          (p2[k] - p0[k]) * t +
          (2 * p0[k] - 5 * p1[k] + 4 * p2[k] - p3[k]) * t * t +
          (3 * p1[k] - p0[k] - 3 * p2[k] + p3[k]) * t * t * t);
      out.push([c(0), c(1)]);
    }
  }
  out.push([...at(FACADE.length - 1)]);
  return out;
})();

/** The facade's half width, a little inside the box's. */
const FACADE_HALF = 0.12;

/** The facade's depth at height `h`, read off its smooth profile. */
function facadeAt(h: number): number {
  for (let i = 0; i + 1 < SMOOTH_FACADE.length; i++) {
    const [d0, g0] = SMOOTH_FACADE[i] ?? [0, 0];
    const [d1, g1] = SMOOTH_FACADE[i + 1] ?? [0, 0];
    if (h >= g0 && h <= g1) return d0 + ((h - g0) / (g1 - g0)) * (d1 - d0);
  }
  throw new Error(`facadeAt: ${String(h)} off the facade`);
}

/** The facade's front over heights `h0` to `h1`: its furthest depth there. */
const facadeOver = (h0: number, h1: number) =>
  Math.max(...[0, 0.25, 0.5, 0.75, 1].map((t) => facadeAt(h0 + (h1 - h0) * t)));

/** How far a slot, the display and the badge stand out past the facade. */
const PROUD = 0.002;

/** The drive slots: width, height and middle height each; the floppy slot last. */
const SLOTS: readonly (readonly [w: number, t: number, h: number])[] = [
  [0.15, 0.012, 0.49],
  [0.15, 0.012, 0.45],
  [0.1, 0.006, 0.41],
];

/**
 * The power button's centre, its radius and its dark collar's, and the
 * two lights beside it.
 */
const POWER = { a: 0.02, h: 0.34, r: 0.013, collar: 0.018 } as const;
const LIGHT = 0.006;
const LIGHT_AS = [0.058, 0.078] as const;

/** The display window and the digits' pixel size. */
const DISPLAY = { a0: -0.07, a1: -0.01, h0: 0.25, h1: 0.29 } as const;
const DIGIT_PX = 0.0055;

/** The turbo button beside the display. */
const TURBO = { a0: 0.01, a1: 0.035, h0: 0.26, h1: 0.28 } as const;

/** The badge plate, and the pixel sizes of the name and the signature. */
const BADGE = { a: 0.065, h0: 0.53, h1: 0.57 } as const;
const BADGE_PX = 0.0025;
const SIGNATURE_PX = 0.0018;

/** A box on the facade over `a0..a1` by `h0..h1`, from inside it to `PROUD` past its front there. */
function onFacade(
  k: Kit,
  a0: number,
  a1: number,
  h0: number,
  h1: number,
  s: Surface,
): number {
  const face = facadeOver(h0, h1) + PROUD;
  k.box(a0, a1, TOWER.front - 0.01, face, h0, h1, s);
  return face;
}

/** The designer tower: box, curved facade, slots, button, lights, display and badge. */
const designerTower: PropRecipe = ({ k, kitAt, s }) => {
  const white = s.tinted(TOWER_WHITE);
  const dark = s.tinted(SLOT_DARK);
  k.bevelBox(
    -TOWER.a,
    TOWER.a,
    TOWER.back,
    TOWER.front,
    0,
    TOWER.top,
    0.015,
    white,
  );
  profileAlong(kitAt, ORIGIN, SMOOTH_FACADE, -FACADE_HALF, FACADE_HALF, white);
  for (const [w, t, h] of SLOTS)
    onFacade(k, -w / 2, w / 2, h - t / 2, h + t / 2, dark);
  // The power button, a short round stud in a dark collar, and its lights.
  const pf = facadeOver(POWER.h - POWER.collar, POWER.h + POWER.collar);
  k.extrude(
    discOutline(POWER.a, POWER.h, POWER.collar, 12),
    pf - 0.005,
    pf + PROUD,
    dark,
  );
  k.extrude(
    discOutline(POWER.a, POWER.h, POWER.r, 12),
    pf,
    pf + 0.008,
    s.tinted(BUTTON_GREY),
  );
  LIGHT_AS.forEach((a, i) => {
    onFacade(
      k,
      a - LIGHT / 2,
      a + LIGHT / 2,
      POWER.h - LIGHT / 2,
      POWER.h + LIGHT / 2,
      s.signal(i === 0 ? POWER_GREEN : DRIVE_AMBER),
    );
  });
  // The display: a dark window and the red digits on it.
  const face = onFacade(
    k,
    DISPLAY.a0,
    DISPLAY.a1,
    DISPLAY.h0,
    DISPLAY.h1,
    dark,
  );
  const digits = textRows(MARKS.towerClock);
  const dw = (digits[0]?.length ?? 0) * DIGIT_PX;
  const red = s.signal(CLOCK_RED);
  pixelBoxes(
    k,
    digits,
    (DISPLAY.a0 + DISPLAY.a1) / 2 - dw / 2,
    (DISPLAY.h0 + DISPLAY.h1) / 2 + (digits.length * DIGIT_PX) / 2,
    DIGIT_PX,
    face,
    face + MARK_PROUD,
    (ch) => (ch === "#" ? red : null),
  );
  onFacade(k, TURBO.a0, TURBO.a1, TURBO.h0, TURBO.h1, s.tinted(BUTTON_GREY));
  // The badge plate near the top, the name on it and the signature under
  // the name's right end.
  const plate = onFacade(
    k,
    -BADGE.a,
    BADGE.a,
    BADGE.h0,
    BADGE.h1,
    s.tinted(BADGE_PLATE),
  );
  const name = textRows(MARKS.towerBadge);
  const nw = (name[0]?.length ?? 0) * BADGE_PX;
  const grey = s.tinted(BADGE_GREY);
  const nameTop = BADGE.h1 - 0.005;
  pixelBoxes(
    k,
    name,
    -nw / 2,
    nameTop,
    BADGE_PX,
    plate,
    plate + MARK_PROUD,
    (ch) => (ch === "#" ? grey : null),
  );
  const sw = (SIGNATURE[0]?.length ?? 0) * SIGNATURE_PX;
  pixelBoxes(
    k,
    SIGNATURE,
    nw / 2 - sw,
    nameTop - name.length * BADGE_PX - 0.003,
    SIGNATURE_PX,
    plate,
    plate + MARK_PROUD,
    (ch) => (ch === "#" ? grey : null),
  );
};

/** The other rare props' recipes. */
export const RARE_RECIPES = {
  "ooze-canisters": oozeCanisters,
  "designer-tower": designerTower,
  "saucer-poster": saucerPoster,
} satisfies Record<
  Extract<PropKind, "ooze-canisters" | "saucer-poster" | "designer-tower">,
  PropRecipe
>;
