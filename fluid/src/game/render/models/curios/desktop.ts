/**
 * The desktop curios' recipes (2.6d): the breadbin computer, the slim
 * computer and the space bricks. Colours and helpers stay in this file,
 * which imports only `common.ts` of the curio batches. The one readable
 * line, the computers' badge (`MARKS.computerBadge`), comes from
 * `marks.ts` (C16) and is drawn with `pixelPanel`, one flat quad per run
 * 1.5 mm proud of its face (`MARK_PROUD`), as the finds draw theirs: a
 * box per run would cost six times the triangles, and the badge's runs
 * beside the sixty-six keys would not fit the curio budget as boxes. The
 * kit has no pitch, so every mark sits on an upright face.
 *
 * - Both computers are a wedge of a case, low at the front and high at the
 *   back, with a full-width keyboard in a recess of its sloped top: four
 *   rows of keys laid out as the original's (the wide control, restore,
 *   return and shift keys) and a long space bar in a fifth row at the
 *   front, each key a tapered cap standing on the recess's dark floor, and
 *   four function keys in a column in their own recess on the right. The
 *   case is one body recessed across its whole width, filled up to its
 *   top again round the recesses (the cheeks, the strip between the
 *   recesses, the front row beside the space bar), so its front and sides
 *   stay single faces and every fill meets its neighbour back to back,
 *   never in one plane facing the same way. A band of the case colour
 *   stays free between the back row and the recess's back wall, so the
 *   wall shows over the keys from where a player stands.
 * - The breadbin computer is the chunky brown-beige wedge with a rounded
 *   back, a seam round its lower shell and darker grooves across its back.
 *   Its keys are dark brown and its function keys light grey. On the
 *   recess's back wall, left, a dark badge carries the lower-case word in
 *   light letters, then five stacked stripes (red at the top, blue at the
 *   bottom, slanted by a step per stripe) and the number, as the
 *   original sets them; right, over the function keys, a grey plate holds
 *   the red power light in a black ring, a steady `s.signal`.
 * - The slim computer is the flatter, deeper, angular off-white wedge with
 *   light cream keys, darker function keys and dark vent lines across its
 *   flat back. Its badge is the same line in dark grey on the case front,
 *   low on the right, with no stripe; its power light sits at the left of
 *   the recess's back wall.
 * - The space bricks stand a small half-built ship of studded toy bricks
 *   (`SHIP_BRICKS`) on the corner of a darker grey baseplate: a light
 *   grey chassis plate, a blue hull brick with a blue nose slope in front
 *   and a raised blue back block behind, a narrow pale yellow canopy
 *   wedge between them that carries the nose's slope on up, two grey
 *   engines behind, stepped grey wings with one plain red and one plain
 *   green stud at their tips, and an antenna with a small dish on the
 *   back block, which is the curio's top. Every uncovered stud position
 *   of every brick carries a six-sided stud (the slopes carry none, as
 *   the original's do not); the baseplate carries them on a much coarser
 *   grid over its free area, the triangle budget's price. Two loose
 *   bricks lie beside the ship, one turned. On the desk beside the plate
 *   stands a small red spaceman, four centimetres tall: legs, hips, a
 *   tapered torso with the planet-and-orbit badge (`SPACE_BADGE`) on its
 *   chest, arms with yellow hands, a yellow head with dark eyes under an
 *   open helmet with no visor, as the original's was, and an air tank on
 *   its back. Nothing glows.
 */

import type { CurioKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import { profileAlong, yawed, type KitAt } from "../common";
import { pixelPanel, textRows } from "../heroes/pixels";
import { MARKS, SPACE_BADGE } from "../marks";
import { curioHalf, type CurioRecipe } from "./common";

/** Every curio in this file is built in this frame, at the origin. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/** How far a mark's quads stand in front of the face they are printed on, in metres (C16). */
const MARK_PROUD = 0.0015;

/** A side profile, `[d, h]` points. */
type Profile = readonly (readonly [number, number])[];

// --- The computers -----------------------------------------------------------

/** The breadbin's dark brown keys: found by the tests, so no other part shares the tint. */
export const KEY_BROWN: Rgb = [0.3, 0.2, 0.14];
/** The breadbin's four light grey function keys: found by the tests, so no other part shares it. */
export const FUNCTION_GREY: Rgb = [0.72, 0.72, 0.7];
/** The badge's five stripes, top to bottom: red, orange, yellow, green and blue. */
export const RAINBOW: readonly Rgb[] = [
  [0.85, 0.15, 0.1],
  [0.95, 0.5, 0.1],
  [0.95, 0.85, 0.15],
  [0.2, 0.65, 0.3],
  [0.15, 0.35, 0.8],
];
/** Both computers' power light: a bright red past the case's colours, the one `s.signal` of each. */
export const POWER_RED: Rgb = [1.0, 0.1, 0.05];
/** Each computer's badge letters: light on the breadbin's dark badge, dark grey on the slim one's case. */
export const BADGE_TEXT: Readonly<
  Record<"breadbin-computer" | "slim-computer", Rgb>
> = {
  "breadbin-computer": [0.85, 0.85, 0.8],
  "slim-computer": [0.35, 0.35, 0.33],
};

/** The breadbin's brown-beige case. */
const BREAD_CASE: Rgb = [0.72, 0.64, 0.5];
/** The breadbin's grooves: its case colour in shade. */
const BREAD_GROOVE: Rgb = [0.52, 0.46, 0.36];
/** The breadbin's recess floor, seen between the keys: a dark brown. */
const BREAD_FLOOR: Rgb = [0.12, 0.09, 0.07];
/** The breadbin's badge field and the power light's plate: a dark warm grey. */
const BADGE_FIELD: Rgb = [0.3, 0.28, 0.26];
/** The power light's ring. */
const LIGHT_RING: Rgb = [0.05, 0.05, 0.05];

/** The slim computer's off-white case. */
const SLIM_CASE: Rgb = [0.9, 0.88, 0.82];
/** The slim computer's light cream keys. */
const SLIM_KEY: Rgb = [0.86, 0.84, 0.78];
/** The slim computer's function keys: a darker warm grey, as the original's. */
const SLIM_FUNCTION: Rgb = [0.64, 0.61, 0.56];
/** The slim computer's vent lines across its back. */
const SLIM_VENT: Rgb = [0.5, 0.49, 0.46];
/** The slim computer's recess floor: a dark grey. */
const SLIM_FLOOR: Rgb = [0.22, 0.22, 0.21];

/** The key pitch along `a` and between rows, and a key cap's width gap and depth. */
const KEY_PITCH = 0.02;
const ROW_PITCH = 0.021;
const KEY_GAP = 0.003;
const KEY_DEPTH = 0.018;
/** A key cap's height over the recess floor, how far its foot sinks into it, and its taper at the top. */
const KEY_HEIGHT = 0.013;
const KEY_SINK = 0.001;
const KEY_TAPER = 0.0015;
/** The gap between the front row and the recess's front edge. */
const LIP_GAP = 0.003;
/** The recess floor's depth under the case top at its front edge and at its back wall. */
const FLOOR_FRONT = 0.006;
const FLOOR_BACK = 0.0125;
/** The band of case colour between the back row and the recess's back wall. */
const BAND = 0.014;
/** The dark floor plate's thickness over the recess floor. */
const FLOOR_PLATE = 0.0008;

/**
 * The keyboard's four rows, back to front, as key widths in key pitches:
 * the number row of sixteen keys; the control key, thirteen letters and
 * the restore key; run stop, shift lock, twelve letters and the wide
 * return; the logo key, the shifts round ten letters and the two cursor
 * keys. Each row is sixteen pitches long.
 */
const KEY_ROWS: readonly (readonly number[])[] = [
  Array<number>(16).fill(1),
  [1.5, ...Array<number>(13).fill(1), 1.5],
  [1, 1, ...Array<number>(12).fill(1), 2],
  [1, 1.5, ...Array<number>(10).fill(1), 1.5, 1, 1],
];
/** The space bar in the front row: its start and its width in key pitches. */
const SPACE_START = 2.5;
const SPACE_WIDTH = 9;
/** The main keys' left edge, and the function keys' column. */
const KEYS_A0 = -0.183;
const FN_A0 = 0.15;
const FN_A1 = 0.18;
/** The main recess and the function keys' recess along `a`. */
const MAIN_RECESS: readonly [number, number] = [-0.186, 0.14];
const FN_RECESS: readonly [number, number] = [0.147, 0.183];
/** The gap between the space bar and the case filled in beside it. */
const SPACE_CLEAR = 0.002;

/** The surfaces this file's helpers draw with. */
interface Tints {
  tinted: (t: Rgb) => Surface;
  signal: (t: Rgb) => Surface;
}

/** One computer's shape and colours. */
interface Computer {
  /** The side profile, front bottom, up the front, over the top to the back bottom. */
  profile: Profile;
  /** The recess's back wall. */
  wall: number;
  caseTint: Rgb;
  keyTint: Rgb;
  fnTint: Rgb;
  floorTint: Rgb;
}

/** The case's top at depth `d`: the profile's top edge, between its vertices. */
function topAt(profile: Profile, d: number): number {
  const top = profile.slice(1, -1);
  for (let i = 0; i + 1 < top.length; i++) {
    const [d0, h0] = top[i] ?? [0, 0];
    const [d1, h1] = top[i + 1] ?? [0, 0];
    if (d0 === d1) continue;
    if (d <= d0 && d >= d1) return h0 + ((d - d0) / (d1 - d0)) * (h1 - h0);
  }
  throw new Error(`topAt: no top over ${String(d)}`);
}

/** Where a computer's keys and recesses lie in depth. */
function layout(c: Computer) {
  // Rows 0 (back) to 4 (the space bar), their centres.
  const lip = c.wall + BAND + KEY_DEPTH + 4 * ROW_PITCH + LIP_GAP;
  const row = (r: number) =>
    lip - LIP_GAP - KEY_DEPTH / 2 - (4 - r) * ROW_PITCH;
  const fnLip = row(3) + KEY_DEPTH / 2 + LIP_GAP;
  const tl = topAt(c.profile, lip);
  const tw = topAt(c.profile, c.wall);
  const floor = (d: number) =>
    tl -
    FLOOR_FRONT +
    ((d - lip) / (c.wall - lip)) * (tw - FLOOR_BACK - (tl - FLOOR_FRONT));
  return { lip, fnLip, row, floor };
}

/** The profile with a recess cut from `lip` back to the wall, its floor `floor`. */
function notched(
  c: Computer,
  lip: number,
  floor: (d: number) => number,
): Profile {
  const front = c.profile.filter(([d]) => d > lip);
  const back = c.profile.filter(([d]) => d < c.wall);
  return [
    ...front,
    [lip, topAt(c.profile, lip)],
    [lip, floor(lip)],
    [c.wall, floor(c.wall)],
    [c.wall, topAt(c.profile, c.wall)],
    ...back,
  ];
}

/**
 * The recess from `front` back to `back` filled up to the case top again:
 * the floor, the front edge, the top with the profile's own vertices
 * between, and the back.
 */
function filled(
  c: Computer,
  front: number,
  back: number,
  floor: (d: number) => number,
): Profile {
  return [
    [front, floor(front)],
    [front, topAt(c.profile, front)],
    ...c.profile.filter(([d], i) => i > 0 && d < front && d > back),
    [back, topAt(c.profile, back)],
    [back, floor(back)],
  ];
}

/**
 * One key cap from `a0` to `a1` centred on `dc`: its foot sunk into the
 * recess floor, its top parallel to the floor and narrower by the taper
 * at the front and the back.
 */
function key(
  kitAt: KitAt,
  floor: (d: number) => number,
  a0: number,
  a1: number,
  dc: number,
  s: Surface,
): void {
  const d0 = dc - KEY_DEPTH / 2;
  const d1 = dc + KEY_DEPTH / 2;
  profileAlong(
    kitAt,
    ORIGIN,
    [
      [d1, floor(d1) - KEY_SINK],
      [d1 - KEY_TAPER, floor(d1 - KEY_TAPER) + KEY_HEIGHT],
      [d0 + KEY_TAPER, floor(d0 + KEY_TAPER) + KEY_HEIGHT],
      [d0, floor(d0) - KEY_SINK],
    ],
    a0,
    a1,
    s,
  );
}

/**
 * A thin strip lying on the case top across `a0..a1`, centred on `d` and
 * `width` deep, standing `proud` over it: a groove or a vent line.
 */
function strip(
  kitAt: KitAt,
  profile: Profile,
  d: number,
  width: number,
  a0: number,
  a1: number,
  s: Surface,
): void {
  const p = d + width / 2;
  const q = d - width / 2;
  profileAlong(
    kitAt,
    ORIGIN,
    [
      [p, topAt(profile, p) - 0.0004],
      [p, topAt(profile, p) + 0.0005],
      [q, topAt(profile, q) + 0.0005],
      [q, topAt(profile, q) - 0.0004],
    ],
    a0,
    a1,
    s,
  );
}

/**
 * A computer's case, recesses, floor plates and keys; returns the recess
 * layout, so the recipe can hang its badge and power light on the wall.
 */
function computer(kitAt: KitAt, s: Tints, hw: number, c: Computer) {
  const shell = s.tinted(c.caseTint);
  const dark = s.tinted(c.floorTint);
  const keyTint = s.tinted(c.keyTint);
  const fnTint = s.tinted(c.fnTint);
  const { lip, fnLip, row, floor } = layout(c);
  const [m0, m1] = MAIN_RECESS;
  const [f0, f1] = FN_RECESS;
  const space = KEYS_A0 + SPACE_START * KEY_PITCH;
  const spaceEnd = space + SPACE_WIDTH * KEY_PITCH - KEY_GAP;
  // The case, recessed across its whole width from the front edge back to
  // the wall, then filled up to its top again round the two recesses:
  // the cheeks, the strip between them, the front row beside the space
  // bar and the front of the function keys' recess.
  profileAlong(kitAt, ORIGIN, notched(c, lip, floor), -hw, hw, shell);
  for (const [a0, a1, back] of [
    [-hw, m0, c.wall],
    [m1, f0, c.wall],
    [f1, hw, c.wall],
    [f0, f1, fnLip],
    [m0, space - SPACE_CLEAR, fnLip],
    [spaceEnd + SPACE_CLEAR, m1, fnLip],
  ] as const)
    profileAlong(kitAt, ORIGIN, filled(c, lip, back, floor), a0, a1, shell);
  // The dark floor plates, under the keys only.
  const back = row(0) - KEY_DEPTH / 2 - 0.001;
  for (const [a0, a1, front] of [
    [m0, m1, lip],
    [f0, f1, fnLip],
  ] as const)
    profileAlong(
      kitAt,
      ORIGIN,
      [
        [front, floor(front)],
        [front, floor(front) + FLOOR_PLATE],
        [back, floor(back) + FLOOR_PLATE],
        [back, floor(back)],
      ],
      a0,
      a1,
      dark,
    );
  // The four rows of main keys, the space bar and the function keys.
  KEY_ROWS.forEach((widths, r) => {
    let a = KEYS_A0;
    for (const w of widths) {
      key(kitAt, floor, a, a + w * KEY_PITCH - KEY_GAP, row(r), keyTint);
      a += w * KEY_PITCH;
    }
    key(kitAt, floor, FN_A0, FN_A1, row(r), fnTint);
  });
  key(kitAt, floor, space, spaceEnd, row(4), keyTint);
  return { floor, top: (d: number) => topAt(c.profile, d) };
}

/**
 * The power light on the recess's back wall at `a`, its middle at `h`: a
 * black ring standing on the wall and the red light proud of it.
 */
function powerLight(
  k: Kit,
  s: Tints,
  wall: number,
  a: number,
  h: number,
): void {
  k.box(
    a - 0.004,
    a + 0.004,
    wall - 0.0005,
    wall + 0.0022,
    h - 0.004,
    h + 0.004,
    s.tinted(LIGHT_RING),
  );
  k.box(
    a - 0.0025,
    a + 0.0025,
    wall + 0.0015,
    wall + 0.0042,
    h - 0.0025,
    h + 0.0025,
    s.signal(POWER_RED),
  );
}

/** The badge line's pixel rows, and its word's and its number's columns. */
const BADGE_ROWS = textRows(MARKS.computerBadge);
const WORD_COLS = MARKS.computerBadge.indexOf(" ") * 4 - 1;
const NUMBER_COL = (MARKS.computerBadge.indexOf(" ") + 1) * 4;

/** The breadbin's side profile over its lower shell: a low front, a sloped top, a rounded back. */
const BREAD_PROFILE: Profile = [
  [0.105, 0.012],
  [0.105, 0.022],
  [0.09, 0.03],
  [-0.02, 0.058],
  [-0.06, 0.07],
  [-0.09, 0.075],
  [-0.105, 0.068],
  [-0.105, 0.012],
];
/** The breadbin's lower shell's top: the seam. */
const BREAD_SEAM = 0.012;
/** The lower shell's inset under the upper, which draws the seam. */
const SEAM_INSET = 0.0015;
/** The breadbin's recess back wall. */
const BREAD_WALL = -0.033;
/** The grooves across the breadbin's back. */
const BREAD_GROOVES: readonly number[] = [
  -0.04, -0.048, -0.056, -0.066, -0.075, -0.084,
];
/** The breadbin's badge: its field along `a`, its pixel, the gaps round the stripes, a stripe's length and its slant per row. */
const BADGE_A: readonly [number, number] = [-0.183, -0.068];
const BADGE_PX = 0.0019;
const STRIPE_GAP = 0.003;
const STRIPE_LEN = 0.02;
const STRIPE_SLANT = 0.0008;
/** Where the breadbin's power light sits along `a`, over the function keys. */
const BREAD_LIGHT_A = 0.172;

const breadbinComputer: CurioRecipe = ({ k, kitAt, s }) => {
  const { hw, hd } = curioHalf("breadbin-computer", 0);
  const c: Computer = {
    profile: BREAD_PROFILE,
    wall: BREAD_WALL,
    caseTint: BREAD_CASE,
    keyTint: KEY_BROWN,
    fnTint: FUNCTION_GREY,
    floorTint: BREAD_FLOOR,
  };
  // The lower shell, set in a little under the upper so the seam shows.
  k.box(
    -hw + SEAM_INSET,
    hw - SEAM_INSET,
    -hd + SEAM_INSET,
    hd - SEAM_INSET,
    0,
    BREAD_SEAM + 0.0005,
    s.tinted(BREAD_CASE),
  );
  const { floor, top } = computer(kitAt, s, hw, c);
  const groove = s.tinted(BREAD_GROOVE);
  for (const d of BREAD_GROOVES)
    strip(kitAt, BREAD_PROFILE, d, 0.0016, -hw + 0.008, hw - 0.008, groove);
  // The badge on the wall's left: the word, the stripes, the number.
  const wall = BREAD_WALL;
  const mid = (floor(wall) + top(wall)) / 2;
  const field = wall + 0.0012;
  const face = field + MARK_PROUD;
  k.box(
    BADGE_A[0],
    BADGE_A[1],
    wall - 0.0005,
    field,
    mid - 0.0055,
    mid + 0.0055,
    s.tinted(BADGE_FIELD),
  );
  const numberCols = (BADGE_ROWS[0]?.length ?? 0) - NUMBER_COL;
  const content =
    (WORD_COLS + numberCols) * BADGE_PX +
    2 * STRIPE_GAP +
    STRIPE_LEN +
    4 * STRIPE_SLANT;
  const left = (BADGE_A[0] + BADGE_A[1]) / 2 - content / 2;
  const h1 = mid + (5 * BADGE_PX) / 2;
  const letters = s.tinted(BADGE_TEXT["breadbin-computer"]);
  pixelPanel(
    k,
    BADGE_ROWS.map((r) => r.slice(0, WORD_COLS)),
    left,
    h1,
    BADGE_PX,
    face,
    (ch) => (ch === "#" ? letters : null),
  );
  const stripes = left + WORD_COLS * BADGE_PX + STRIPE_GAP;
  RAINBOW.forEach((tint, i) => {
    const a0 = stripes + (4 - i) * STRIPE_SLANT;
    k.panel(
      a0,
      a0 + STRIPE_LEN,
      face,
      h1 - (i + 1) * BADGE_PX,
      h1 - i * BADGE_PX,
      s.tinted(tint),
    );
  });
  pixelPanel(
    k,
    BADGE_ROWS.map((r) => r.slice(NUMBER_COL)),
    stripes + STRIPE_LEN + 4 * STRIPE_SLANT + STRIPE_GAP,
    h1,
    BADGE_PX,
    face,
    (ch) => (ch === "#" ? letters : null),
  );
  // The power light's plate and the light, over the function keys.
  k.box(
    FN_RECESS[0] + 0.002,
    FN_RECESS[1] - 0.002,
    wall - 0.0005,
    wall + 0.0012,
    mid - 0.0045,
    mid + 0.0045,
    s.tinted(BADGE_FIELD),
  );
  powerLight(k, s, wall + 0.0012 - 0.0005, BREAD_LIGHT_A, mid);
};

/** The slim computer's side profile: a thin front, one long slope, a flat angular back. */
const SLIM_PROFILE: Profile = [
  [0.1235, 0],
  [0.1235, 0.018],
  [0.08, 0.03],
  [-0.07, 0.062],
  [-0.125, 0.07],
  [-0.125, 0],
];
/** The slim computer's recess back wall. */
const SLIM_WALL = -0.042;
/** The slim computer's vent lines across its flat back, and their half width along `a`. */
const SLIM_VENTS: readonly number[] = Array.from(
  { length: 9 },
  (_, i) => -0.077 - i * 0.0052,
);
const VENT_HW = 0.175;
/** The slim computer's badge: its left edge, its top and its pixel, on the case front. */
const SLIM_BADGE_A = 0.09;
const SLIM_BADGE_TOP = 0.012;
const SLIM_BADGE_PX = 0.0016;
/** Where the slim computer's power light sits along `a`, at the left. */
const SLIM_LIGHT_A = -0.17;

const slimComputer: CurioRecipe = ({ k, kitAt, s }) => {
  const { hw } = curioHalf("slim-computer", 0);
  const c: Computer = {
    profile: SLIM_PROFILE,
    wall: SLIM_WALL,
    caseTint: SLIM_CASE,
    keyTint: SLIM_KEY,
    fnTint: SLIM_FUNCTION,
    floorTint: SLIM_FLOOR,
  };
  const { floor, top } = computer(kitAt, s, hw, c);
  const vent = s.tinted(SLIM_VENT);
  for (const d of SLIM_VENTS)
    strip(kitAt, SLIM_PROFILE, d, 0.0016, -VENT_HW, VENT_HW, vent);
  // The badge, dark grey on the case front, low on the right.
  const letters = s.tinted(BADGE_TEXT["slim-computer"]);
  pixelPanel(
    k,
    BADGE_ROWS,
    SLIM_BADGE_A,
    SLIM_BADGE_TOP,
    SLIM_BADGE_PX,
    (SLIM_PROFILE[0]?.[0] ?? 0) + MARK_PROUD,
    (ch) => (ch === "#" ? letters : null),
  );
  powerLight(
    k,
    s,
    SLIM_WALL,
    SLIM_LIGHT_A,
    (floor(SLIM_WALL) + top(SLIM_WALL)) / 2,
  );
};

// --- Space bricks ------------------------------------------------------------

/**
 * The bricks' blue and light grey, the baseplate's darker grey: the two
 * greys lie far enough apart that the grey wings and plates stand off the
 * baseplate under the station's light.
 */
const BRICK_BLUE: Rgb = [0.1, 0.3, 0.7];
const BRICK_GREY: Rgb = [0.78, 0.8, 0.8];
const BASEPLATE: Rgb = [0.4, 0.42, 0.42];
/** The canopy: a plain pale yellow tint, neither lit nor clear. */
const CANOPY: Rgb = [0.95, 0.85, 0.3];
/** The wing tips' clear-coloured studs: plain red and green tints. */
const STUD_RED: Rgb = [0.85, 0.2, 0.15];
const STUD_GREEN: Rgb = [0.25, 0.75, 0.3];
/** The figure's red suit and helmet: found by the tests, so no other part shares it. */
export const FIGURE_RED: Rgb = [0.8, 0.12, 0.1];
/** The figure's face and hands. */
const FIGURE_YELLOW: Rgb = [0.95, 0.8, 0.2];
/** The figure's eyes. */
const FIGURE_EYE: Rgb = [0.05, 0.05, 0.05];
/** The chest badge's planet: found by the tests, so no other part shares it. */
export const BADGE_YELLOW: Rgb = [0.95, 0.8, 0.15];
/** The chest badge's orbit: white, as the original's, over the red suit. */
const BADGE_ORBIT: Rgb = [0.95, 0.95, 0.92];

/** The brick grid: a stud pitch, a plate's and a brick's height. */
const STUD = 0.008;
const PLATE_H = 0.0032;
const BRICK_H = 0.0096;
/** A stud: its radius, its height and its sides. */
const STUD_R = 0.0024;
const STUD_H = 0.0017;
const STUD_SIDES = 6;
/** The baseplate: its top, its extent and the pitch of its coarser stud grid. */
const PLATE_TOP = 0.003;
const PLATE_A: readonly [number, number] = [-0.125, 0.07];
const PLATE_D = 0.075;
const PLATE_GRID = 0.04;

/** One brick's box: `a`, `d` and `h` ranges. */
interface Brick {
  a0: number;
  a1: number;
  d0: number;
  d1: number;
  h0: number;
  h1: number;
}

/** A brick of `w` by `l` studs from `(a0, d0)`, its foot at `h0`, `h` tall. */
const brickAt = (
  a0: number,
  d0: number,
  w: number,
  l: number,
  h0: number,
  h: number,
): Brick => ({
  a0,
  a1: a0 + w * STUD,
  d0,
  d1: d0 + l * STUD,
  h0,
  h1: h0 + h,
});

/** The first layer (plates on the baseplate), the hull's and the back block's heights. */
const L0 = PLATE_TOP;
const L1 = L0 + PLATE_H;
const L2 = L1 + BRICK_H;

/**
 * The ship's bricks, the nose towards `+a`: the grey chassis plate, the
 * blue hull brick on it and the blue back block on the hull, the two
 * grey wing plates beside the chassis and their stepped tips behind. The
 * blue nose slope and the canopy are wedges, not bricks, and carry no
 * studs, as sloped bricks do not. Each brick keeps some of its top
 * uncovered, so each carries studs.
 */
export const SHIP_BRICKS: readonly Brick[] = [
  brickAt(-0.096, -0.016, 11, 4, L0, PLATE_H),
  brickAt(-0.088, -0.016, 5, 4, L1, BRICK_H),
  brickAt(-0.088, -0.016, 2, 4, L2, BRICK_H),
  brickAt(-0.088, 0.016, 4, 2, L0, PLATE_H),
  brickAt(-0.088, -0.032, 4, 2, L0, PLATE_H),
  brickAt(-0.088, 0.032, 1, 1, L0, PLATE_H),
  brickAt(-0.088, -0.04, 1, 1, L0, PLATE_H),
];
/** Each ship brick's colour, in `SHIP_BRICKS` order. */
const SHIP_TINTS: readonly Rgb[] = [
  BRICK_GREY,
  BRICK_BLUE,
  BRICK_BLUE,
  BRICK_GREY,
  BRICK_GREY,
  BRICK_GREY,
  BRICK_GREY,
];
/** The blue nose slope on the chassis in front of the hull: `[a, h]` outline, across the hull's width. */
const NOSE_OUTLINE: readonly (readonly [number, number])[] = [
  [-0.0485, L1],
  [-0.008, L1],
  [-0.008, L1 + 0.0015],
  [-0.0485, L2],
];
const NOSE_D: readonly [number, number] = [-0.016, 0.016];
/** The canopy on the hull in front of the back block, rising on from the nose's slope: `[a, h]` outline and its `d` range. */
const CANOPY_OUTLINE: readonly (readonly [number, number])[] = [
  [-0.0725, L2],
  [-0.048, L2],
  [-0.058, L2 + 0.0034],
  [-0.066, L2 + 0.0052],
  [-0.0725, L2 + 0.0056],
];
const CANOPY_D: readonly [number, number] = [-0.008, 0.008];
/** The two engines behind the back block: their `d`, their radius and their length. */
const ENGINE_D = 0.008;
const ENGINE_R = 0.0035;
const ENGINE_LEN = 0.01;
/** The antenna: its place on the back block, its rod's radius, and the dish's radius and thickness under the curio's top. */
const ANTENNA: readonly [number, number] = [-0.084, 0.012];
const ROD_R = 0.0016;
const DISH_R = 0.0065;
const DISH_T = 0.0025;
/** The stud positions of the wing tips that are clear red and green. */
const TIP_STUDS: readonly (readonly [number, number, Rgb])[] = [
  [-0.084, 0.036, STUD_RED],
  [-0.084, -0.036, STUD_GREEN],
];

/** The loose bricks: an upright blue two by two, and a grey one by two plate turned on the baseplate. */
const LOOSE_BRICK = brickAt(0.022, 0.026, 2, 2, L0, BRICK_H);
const LOOSE_PLATE_AT: readonly [number, number, number] = [0.038, -0.032, 0.5];

/** The figure: where it stands, and its parts' heights. */
const FIGURE_A = 0.09;
const FIGURE_D = 0;
const LEGS_TOP = 0.0115;
const HIPS_TOP = 0.0135;
const TORSO_TOP = 0.0265;
const HEAD_TOP = 0.035;
const HELMET_TOP = 0.0395;
/** The chest badge's pixel. */
const SPACE_PX = 0.0014;

/**
 * A stud on a top face at `(a, d)`, its foot at `h`: a six-sided post with
 * a flat top and no bottom, which the face under it hides.
 */
function stud(k: Kit, a: number, d: number, h: number, s: Surface): void {
  k.lathe(
    a,
    d,
    [
      [STUD_R, h],
      [STUD_R, h + STUD_H],
      [0, h + STUD_H],
    ],
    STUD_SIDES,
    s,
  );
}

/** Whether `(a, d)` lies inside brick `b`'s footprint. */
const under = (b: Brick, a: number, d: number) =>
  a > b.a0 && a < b.a1 && d > b.d0 && d < b.d1;

const spaceBricks: CurioRecipe = ({ k, kitAt, s }) => {
  const { top } = curioHalf("space-bricks", 0);
  const grey = s.tinted(BASEPLATE);
  // The baseplate.
  k.box(PLATE_A[0], PLATE_A[1], -PLATE_D, PLATE_D, 0, PLATE_TOP, grey);
  // The ship's bricks, with studs on every uncovered stud position; the
  // nose slope and the canopy cover the positions under them.
  const wedge = (
    outline: readonly (readonly [number, number])[],
    [d0, d1]: readonly [number, number],
    h0: number,
  ): Brick => ({
    a0: Math.min(...outline.map(([a]) => a)),
    a1: Math.max(...outline.map(([a]) => a)),
    d0,
    d1,
    h0,
    h1: h0,
  });
  const covers = [
    ...SHIP_BRICKS,
    wedge(NOSE_OUTLINE, NOSE_D, L1),
    wedge(CANOPY_OUTLINE, CANOPY_D, L2),
  ];
  SHIP_BRICKS.forEach((b, i) => {
    const tint = s.tinted(SHIP_TINTS[i] ?? BRICK_GREY);
    k.box(b.a0, b.a1, b.d0, b.d1, b.h0, b.h1, tint);
    for (let a = b.a0 + STUD / 2; a < b.a1; a += STUD)
      for (let d = b.d0 + STUD / 2; d < b.d1; d += STUD) {
        if (covers.some((c) => c.h0 === b.h1 && under(c, a, d))) continue;
        if (Math.hypot(a - ANTENNA[0], d - ANTENNA[1]) < 1e-9) continue;
        const tip = TIP_STUDS.find(
          ([ta, td]) => Math.hypot(a - ta, d - td) < 1e-9,
        );
        stud(k, a, d, b.h1, tip ? s.tinted(tip[2]) : tint);
      }
  });
  const blue = s.tinted(BRICK_BLUE);
  const lightGrey = s.tinted(BRICK_GREY);
  k.extrude(NOSE_OUTLINE, NOSE_D[0], NOSE_D[1], blue);
  k.extrude(CANOPY_OUTLINE, CANOPY_D[0], CANOPY_D[1], s.tinted(CANOPY));
  // The two engines behind the back block.
  const back = SHIP_BRICKS[2]?.a0 ?? 0;
  for (const d of [-ENGINE_D, ENGINE_D])
    k.cylinderAlong(
      back - ENGINE_LEN,
      back + 0.0005,
      d,
      L2 + BRICK_H / 2,
      ENGINE_R,
      6,
      lightGrey,
    );
  // The antenna on the back block, its dish the curio's top.
  k.cylinder(
    ANTENNA[0],
    ANTENNA[1],
    L2 + BRICK_H,
    top - DISH_T,
    ROD_R,
    4,
    lightGrey,
  );
  k.cylinder(ANTENNA[0], ANTENNA[1], top - DISH_T, top, DISH_R, 8, lightGrey);
  // The loose bricks: the blue two by two and the turned grey plate.
  const lb = LOOSE_BRICK;
  k.box(lb.a0, lb.a1, lb.d0, lb.d1, lb.h0, lb.h1, blue);
  for (let a = lb.a0 + STUD / 2; a < lb.a1; a += STUD)
    for (let d = lb.d0 + STUD / 2; d < lb.d1; d += STUD)
      stud(k, a, d, lb.h1, blue);
  const [pa, pd, turn] = LOOSE_PLATE_AT;
  const kp = kitAt(yawed(ORIGIN, pa, pd, turn));
  kp.box(-STUD, STUD, -STUD / 2, STUD / 2, L0, L1, lightGrey);
  for (const i of [-0.5, 0.5]) stud(kp, i * STUD, 0, L1, lightGrey);
  // The baseplate's studs on a coarser grid, clear of the ship and the
  // loose bricks.
  const clear = (a: number, d: number) =>
    !(a > -0.106 && a < 0.004 && d > -0.046 && d < 0.046) &&
    !(a > 0.014 && a < 0.058 && d > -0.052 && d < 0.05);
  for (let a = PLATE_A[0] + PLATE_GRID / 2; a < PLATE_A[1]; a += PLATE_GRID)
    for (let d = -PLATE_D + PLATE_GRID / 2; d < PLATE_D; d += PLATE_GRID)
      if (clear(a, d)) stud(k, a, d, PLATE_TOP, grey);
  // The figure on the desk beside the plate, facing +d.
  figure(k, s);
};

/** The small red spaceman at `FIGURE_A`, facing `+d`. */
function figure(k: Kit, s: Tints): void {
  const red = s.tinted(FIGURE_RED);
  const yellow = s.tinted(FIGURE_YELLOW);
  const x = FIGURE_A;
  const z = FIGURE_D;
  // Legs, hips and the tapered torso.
  k.box(x - 0.0078, x - 0.0003, z - 0.0035, z + 0.0045, 0, LEGS_TOP, red);
  k.box(x + 0.0003, x + 0.0078, z - 0.0035, z + 0.0045, 0, LEGS_TOP, red);
  k.box(x - 0.0078, x + 0.0078, z - 0.0035, z + 0.004, LEGS_TOP, HIPS_TOP, red);
  k.extrude(
    [
      [x - 0.0078, HIPS_TOP],
      [x + 0.0078, HIPS_TOP],
      [x + 0.0058, TORSO_TOP],
      [x - 0.0058, TORSO_TOP],
    ],
    z - 0.0035,
    z + 0.0035,
    red,
  );
  // The arms hanging at its sides, and the yellow hands.
  for (const side of [-1, 1]) {
    const a0 = x + side * 0.0062;
    const a1 = x + side * 0.0098;
    k.box(a0, a1, z - 0.002, z + 0.0035, 0.017, TORSO_TOP - 0.001, red);
    k.box(
      x + side * 0.0068,
      x + side * 0.0092,
      z - 0.0005,
      z + 0.0045,
      0.0135,
      0.0172,
      yellow,
    );
  }
  // The head, its eyes, and the open helmet round its back and top.
  k.cylinder(x, z, TORSO_TOP, HEAD_TOP, 0.0045, 8, yellow);
  const eye = s.tinted(FIGURE_EYE);
  for (const side of [-1, 1])
    k.box(
      x + side * 0.0017 - 0.0005,
      x + side * 0.0017 + 0.0005,
      z + 0.0036,
      z + 0.0047,
      0.0312,
      0.0322,
      eye,
    );
  k.box(
    x - 0.0056,
    x + 0.0056,
    z - 0.0056,
    z + 0.0008,
    TORSO_TOP + 0.001,
    HELMET_TOP,
    red,
  );
  k.box(
    x - 0.0056,
    x + 0.0056,
    z + 0.0008,
    z + 0.0052,
    HEAD_TOP - 0.0008,
    HELMET_TOP,
    red,
  );
  for (const side of [-1, 1])
    k.box(
      x + side * 0.0048 - 0.0008,
      x + side * 0.0048 + 0.0008,
      z + 0.0008,
      z + 0.0046,
      TORSO_TOP + 0.001,
      HEAD_TOP,
      red,
    );
  // The air tank on its back.
  k.box(
    x - 0.0055,
    x + 0.0055,
    z - 0.0072,
    z - 0.0035,
    0.015,
    TORSO_TOP - 0.0005,
    red,
  );
  // The planet-and-orbit badge on its chest.
  const w = (SPACE_BADGE[0]?.length ?? 0) * SPACE_PX;
  const planet = s.tinted(BADGE_YELLOW);
  const orbit = s.tinted(BADGE_ORBIT);
  pixelPanel(
    k,
    SPACE_BADGE,
    x - w / 2,
    TORSO_TOP - 0.0022,
    SPACE_PX,
    z + 0.0035 + MARK_PROUD,
    (ch) => (ch === "p" ? planet : ch === "o" ? orbit : null),
  );
}

/** The desktop curios' recipes. */
export const DESKTOP_RECIPES = {
  "breadbin-computer": breadbinComputer,
  "slim-computer": slimComputer,
  "space-bricks": spaceBricks,
} satisfies Record<
  Extract<CurioKind, "breadbin-computer" | "slim-computer" | "space-bricks">,
  CurioRecipe
>;
