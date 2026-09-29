/**
 * The finds' recipes (2.6d): the treasure radar, the capsule case and the
 * reactor case. Colours and helpers stay in this file, which imports only
 * `common.ts` of the curio batches. Every readable mark comes from
 * `marks.ts` (C16) and is drawn with `pixelPanel`, one flat quad per run
 * 1.5 mm proud of its face (`MARK_PROUD`): a box per run would cost six
 * times the triangles, and the capsules' and the plaque's runs would not
 * fit the curio budget as boxes.
 *
 * - The treasure radar is a round white handheld the size of a pocket
 *   watch lying screen up: a two-tone body (a greyer lower rim under a
 *   white shell), a white bezel ring round a glowing green screen crossed
 *   by a five-by-five grid of darker green lines, and a round button on a
 *   short stem at its top edge (`-d`), like a stopwatch's crown. Four
 *   yellow-orange dots sit on grid crossings, one at the centre, and blink
 *   in groups 0 to 3 of the `status` bank, each on a steady dim orange
 *   base (`s.signal`) that it never shows darker than, so a dot at the
 *   strobe's low reads dim orange, never dark; the screen is a steady
 *   `s.glow`. No text.
 * - The capsule case is a light grey tray with a dark foam top and a small
 *   latch at its front, its lid standing open upright at the back on a
 *   hinge barrel: grey outside, white inside, with a whiter label field
 *   carrying the maker's round C over its word (`CAPSULE_LOGO`,
 *   `MARKS.capsuleWord`) in dark blue. Five pill-shaped capsules stand in
 *   a row in the foam, each its own colour (`CAPSULE_TINTS`) over a white
 *   base ring, a small dark push button on top, and on a flat front facet
 *   a small round C over its number (`MARKS.capsuleNumbers`). No glow.
 * - The reactor case is a black plinth with a brass plaque over its whole
 *   front carrying `MARKS.reactorPlaque` in dark letters, cut into three
 *   lines at its spaces (`PLAQUE_LINES`) so it reads from a close view. On
 *   the plinth a black post holds the palm-sized reactor upright, facing
 *   `+d`: a deep dark housing, a grey metal ring, ten glowing wedge
 *   segments round a bright blue-white core disc. The core breathes in
 *   group 0 and the segments in group 1 of the `breathe` bank. A lattice
 *   of twelve thin pale bars draws the glass box round it, since the
 *   renderer draws no translucent surface.
 */

import type { CurioKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import { discOutline, yawed, type KitAt } from "../common";
import { MARK_PROUD, pixelPanel, textRows } from "../heroes/pixels";
import { CAPSULE_LOGO, MARK_BLUE, MARKS } from "../marks";
import { curioHalf, type CurioRecipe } from "./common";

/** Every curio in this file is built in this frame, at the origin. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/**
 * A short cylinder whose axis runs along `d` from `d0` to `d1` at `(a,
 * h)`: `cylinderAlong` in a frame yawed a quarter, whose `along` is the
 * old `inward`. For the radar's crown.
 */
function cylinderAlongD(
  kitAt: KitAt,
  a: number,
  h: number,
  d0: number,
  d1: number,
  radius: number,
  sides: number,
  s: Surface,
): void {
  kitAt(yawed(ORIGIN, a, 0, Math.PI / 2)).cylinderAlong(
    d0,
    d1,
    0,
    h,
    radius,
    sides,
    s,
  );
}

// --- Treasure radar ----------------------------------------------------------

/** The radar's shell: a clean white. */
const RADAR_WHITE: Rgb = [0.92, 0.92, 0.9];
/** The radar's lower rim: the same white in shade, so the shell reads as two halves. */
const RADAR_RIM: Rgb = [0.74, 0.75, 0.74];
/** The screen's glowing green. */
const RADAR_SCREEN: Rgb = [0.15, 0.55, 0.25];
/** The grid lines: a darker green over the screen. */
const RADAR_GRID: Rgb = [0.04, 0.26, 0.1];
/**
 * A dot's steady base: a dim orange `s.signal` under each blinking dot,
 * a little wider than the dot, so the dot never reads dark.
 */
const DOT_BASE: Rgb = [0.5, 0.24, 0.03];
/**
 * The blinking dots: about `DOT_BASE` over `BLINK_LOW` (0.15), past 1
 * on purpose. A blink light is its tint times its gain, so at the strobe's
 * low the dot shows its base's own dim orange, and at the flash the
 * renderer's clamp takes it to a bright yellow-orange (red and green both
 * at 1, blue low).
 */
const RADAR_DOT: Rgb = [3.4, 1.6, 0.2];

/** The radar's body centre in plan, a little forward so the crown fits behind it. */
const RADAR_D = 0.005;
/** The body's radius and the screen's. */
const RADAR_R = 0.04;
const SCREEN_R = 0.032;
/** The shell's top, the screen's top and the grid's top. */
const SHELL_TOP = 0.022;
const SCREEN_TOP = 0.0235;
const GRID_TOP = 0.0245;
/** The grid's line spacing and width: five lines each way. */
const GRID_STEP = 0.011;
const GRID_W = 0.0014;
/** A dot's side, its bottom (sunk in its base) and its top. */
const DOT = 0.0065;
const DOT_BOTTOM = 0.025;
const DOT_TOP = 0.0266;
/** A dot's base: its side and its top, under the dot's top so no face is shared. */
const DOT_BASE_SIDE = 0.0075;
const DOT_BASE_TOP = 0.0255;
/** The dots, `[a, d]` from the screen's centre in grid steps, the first at the centre. */
const DOTS: readonly (readonly [number, number])[] = [
  [0, 0],
  [1, -1],
  [-2, 1],
  [1, 2],
];

const treasureRadar: CurioRecipe = ({ k, kitAt, s }) => {
  const white = s.tinted(RADAR_WHITE);
  // The shell: a greyer lower rim, then the white upper half.
  k.lathe(
    0,
    RADAR_D,
    [
      [0, 0],
      [RADAR_R - 0.004, 0],
      [RADAR_R, 0.004],
      [RADAR_R, 0.009],
    ],
    16,
    s.tinted(RADAR_RIM),
  );
  k.lathe(
    0,
    RADAR_D,
    [
      [RADAR_R, 0.009],
      [RADAR_R, 0.019],
      [RADAR_R - 0.003, SHELL_TOP],
      [0, SHELL_TOP],
    ],
    16,
    white,
  );
  // The screen, glowing, and its grid.
  k.cylinder(
    0,
    RADAR_D,
    SHELL_TOP,
    SCREEN_TOP,
    SCREEN_R,
    16,
    s.glow(RADAR_SCREEN),
  );
  const grid = s.tinted(RADAR_GRID);
  const w = GRID_W / 2;
  for (let i = -2; i <= 2; i++) {
    const o = i * GRID_STEP;
    const half = Math.sqrt(SCREEN_R * SCREEN_R - o * o) - 0.001;
    k.box(
      o - w,
      o + w,
      RADAR_D - half,
      RADAR_D + half,
      SCREEN_TOP,
      GRID_TOP,
      grid,
    );
    k.box(
      -half,
      half,
      RADAR_D + o - w,
      RADAR_D + o + w,
      SCREEN_TOP,
      GRID_TOP,
      grid,
    );
  }
  // The dots on grid crossings, each its own status group, each on its
  // steady dim orange base.
  const c = DOT / 2;
  const cb = DOT_BASE_SIDE / 2;
  DOTS.forEach(([ga, gd], i) => {
    const a = ga * GRID_STEP;
    const d = RADAR_D + gd * GRID_STEP;
    k.box(
      a - cb,
      a + cb,
      d - cb,
      d + cb,
      SCREEN_TOP,
      DOT_BASE_TOP,
      s.signal(DOT_BASE),
    );
    k.box(
      a - c,
      a + c,
      d - c,
      d + c,
      DOT_BOTTOM,
      DOT_TOP,
      s.blink(RADAR_DOT, i),
    );
  });
  // The bezel ring round the screen, its top the curio's top.
  k.ring(0, RADAR_D, 0.026, 0.035, 0.004, 6, 16, white, "up");
  // The crown at the top edge: a short stem and a round button.
  const back = RADAR_D - RADAR_R;
  cylinderAlongD(
    kitAt,
    0,
    0.011,
    back + 0.002,
    back - 0.006,
    0.0038,
    8,
    s.tinted(RADAR_RIM),
  );
  cylinderAlongD(
    kitAt,
    0,
    0.011,
    back - 0.006,
    back - 0.015,
    0.0068,
    10,
    white,
  );
};

// --- Capsule case ------------------------------------------------------------

/**
 * The five capsules' colours, left to right: red, yellow, blue, green and
 * white. No other part of the case shares one, so a capsule is found by
 * its tint.
 */
export const CAPSULE_TINTS: readonly Rgb[] = [
  [0.85, 0.2, 0.15],
  [0.95, 0.8, 0.2],
  [0.2, 0.45, 0.85],
  [0.3, 0.7, 0.3],
  [0.92, 0.92, 0.9],
];

/** The tray and the lid's outside: a light grey. */
const CASE_GREY: Rgb = [0.72, 0.73, 0.75];
/** The foam: a dark grey. */
const CASE_FOAM: Rgb = [0.2, 0.2, 0.22];
/** The latch, the hinge and the capsules' buttons: a darker grey. */
const CASE_DARK: Rgb = [0.34, 0.35, 0.37];
/** The lid's inside: white, a shade off every capsule's tint. */
const LID_WHITE: Rgb = [0.96, 0.96, 0.94];
/** The label field behind the mark: the whitest white. */
const LABEL_WHITE: Rgb = [0.99, 0.99, 0.98];
/** The capsules' base rings: white, a shade off the white capsule's tint. */
const RING_WHITE: Rgb = [0.97, 0.97, 0.95];

/** Where the capsules stand: their `a` positions and their `d`. */
const CAPSULE_AS: readonly number[] = [-0.068, -0.034, 0, 0.034, 0.068];
const CAPSULE_D = 0.005;
/** The capsule's radius at its widest, and its facets. */
const CAPSULE_R = 0.013;
const CAPSULE_SIDES = 8;
/** The capsule's profile, `[r, h]`, bottom to top: a pill with a domed top. */
const CAPSULE_PROFILE: readonly (readonly [number, number])[] = [
  [0, 0.03],
  [0.012, 0.033],
  [CAPSULE_R, 0.045],
  [CAPSULE_R, 0.085],
  [0.012, 0.095],
  [0, 0.098],
];
/** The depth of a capsule's flat front facet: the lathe is turned half a facet so a face, not an edge, looks at `+d`. */
const CAPSULE_FACE = CAPSULE_D + CAPSULE_R * Math.cos(Math.PI / CAPSULE_SIDES);
/** A capsule's mark: the round C's pixel and top, the number's pixel and top. */
const CAP_LOGO_PX = 0.0008;
const CAP_LOGO_TOP = 0.09;
const CAP_NUM_PX = 0.0018;
const CAP_NUM_TOP = 0.078;
/** The lid's mark: the round C's pixel and top, the word's pixel and top. */
const LID_LOGO_PX = 0.0045;
const LID_LOGO_TOP = 0.1495;
const LID_WORD_PX = 0.0022;
const LID_WORD_TOP = 0.097;

/** A picture's width in metres at pixel size `px`. */
const widthOf = (rows: readonly string[], px: number) =>
  (rows[0]?.length ?? 0) * px;

/** Draws `rows` centred on `a` at depth `d`, row 0's top at `h1`, lit cells in `s`. */
function markAt(
  k: Kit,
  rows: readonly string[],
  a: number,
  h1: number,
  px: number,
  d: number,
  s: Surface,
): void {
  pixelPanel(k, rows, a - widthOf(rows, px) / 2, h1, px, d, (ch) =>
    ch === "." ? null : s,
  );
}

const capsuleCase: CurioRecipe = ({ k, kitAt, s }) => {
  const { hw, hd, top } = curioHalf("capsule-case", 0);
  const grey = s.tinted(CASE_GREY);
  const dark = s.tinted(CASE_DARK);
  const blue = s.tinted(MARK_BLUE);
  const trayBack = -0.06;
  const trayFront = hd - 0.002;
  // The tray, its foam top and the latch at its front.
  k.bevelBox(-hw, hw, trayBack, trayFront, 0, 0.03, 0.004, grey);
  k.box(
    -hw + 0.005,
    hw - 0.005,
    trayBack + 0.005,
    trayFront - 0.005,
    0.03,
    0.035,
    s.tinted(CASE_FOAM),
  );
  k.box(-0.012, 0.012, trayFront - 0.002, hd, 0.012, 0.026, dark);
  // The capsules, each turned half a facet so a flat face looks forward.
  CAPSULE_AS.forEach((a, i) => {
    const tint = CAPSULE_TINTS[i] ?? RING_WHITE;
    const kc = kitAt(yawed(ORIGIN, a, CAPSULE_D, Math.PI / CAPSULE_SIDES));
    kc.lathe(0, 0, CAPSULE_PROFILE, CAPSULE_SIDES, s.tinted(tint));
    kc.lathe(
      0,
      0,
      [
        [CAPSULE_R + 0.0002, 0.03],
        [CAPSULE_R + 0.0002, 0.05],
      ],
      CAPSULE_SIDES,
      s.tinted(RING_WHITE),
    );
    k.cylinder(a, CAPSULE_D, 0.095, 0.102, 0.004, 8, dark);
    const front = CAPSULE_FACE + MARK_PROUD;
    markAt(k, CAPSULE_LOGO, a, CAP_LOGO_TOP, CAP_LOGO_PX, front, blue);
    markAt(
      k,
      textRows(MARKS.capsuleNumbers[i] ?? ""),
      a,
      CAP_NUM_TOP,
      CAP_NUM_PX,
      front,
      blue,
    );
  });
  // The lid, open upright at the back: grey outside, white inside.
  k.box(-hw, hw, -hd, -0.065, 0.03, top, grey);
  k.box(-hw, hw, -0.065, trayBack, 0.03, top, s.tinted(LID_WHITE));
  k.cylinderAlong(-hw + 0.005, hw - 0.005, -0.065, 0.03, 0.004, 8, dark);
  // The label field and the mark on the lid's inner face.
  const field = trayBack + 0.0015;
  k.box(-0.062, 0.062, trayBack, field, 0.083, 0.152, s.tinted(LABEL_WHITE));
  markAt(
    k,
    CAPSULE_LOGO,
    0,
    LID_LOGO_TOP,
    LID_LOGO_PX,
    field + MARK_PROUD,
    blue,
  );
  markAt(
    k,
    textRows(MARKS.capsuleWord),
    0,
    LID_WORD_TOP,
    LID_WORD_PX,
    field + MARK_PROUD,
    blue,
  );
};

// --- Reactor case ------------------------------------------------------------

/** The plinth and the post: near black. */
const CASING: Rgb = [0.05, 0.05, 0.05];
/** The plinth's top slab and the reactor's housing: black a shade lighter, so the edges read. */
const CASING_TOP: Rgb = [0.11, 0.11, 0.12];
/** The reactor's ring: a dark grey metal. */
const REACTOR_RING: Rgb = [0.42, 0.43, 0.46];
/** The core's bright blue-white. */
const CORE: Rgb = [0.55, 0.8, 0.98];
/** The segments: the core's blue, a little deeper. */
const SEGMENT: Rgb = [0.4, 0.7, 0.95];

/** The glass lattice's pale tint: found by the tests, so no other part shares it. */
export const GLASS_EDGE: Rgb = [0.8, 0.88, 0.92];
/** The plaque's pale brass: found by the tests, so no other part shares it. */
export const PLAQUE: Rgb = [0.75, 0.65, 0.4];
/** The plaque's letters: a dark engraved brown on the brass. */
export const PLAQUE_TEXT: Rgb = [0.14, 0.11, 0.06];

/** The plinth's top, and the top slab the glass stands on. */
const PLINTH_TOP = 0.07;
const SLAB_TOP = 0.072;
/** The plaque: its half width, its bottom and top, its back and its face. */
const PLAQUE_HW = 0.075;
const PLAQUE_H0 = 0.006;
const PLAQUE_H1 = 0.066;
const PLAQUE_BACK = 0.077;
const PLAQUE_FACE = 0.0785;
/** The plaque's letters' pixel, and the gap between its lines in pixels. */
const PLAQUE_PX = 0.0033;
const LINE_GAP = 1;
/** The reactor's centre height and its ring's radius and tube. */
const REACTOR_H = 0.14;
const RING_R = 0.038;
const RING_TUBE = 0.009;
/** The reactor's round housing behind its ring: its radius and its back, so it reads as deep as the original from the side. */
const HOUSING_R = 0.042;
const HOUSING_BACK = -0.024;
/** The glass box's half width and its bars' side. */
const GLASS_HALF = 0.075;
const BAR = 0.003;

/**
 * The line cut into its lines at spaces: every way of cutting it at
 * `lines - 1` of its spaces, the one whose longest line is shortest.
 * Returns each line's first and last-plus-one character index.
 */
function lineBreaks(text: string, lines: number): [number, number][] {
  const spaces = [...text].flatMap((c, i) => (c === " " ? [i] : []));
  let best: [number, number][] = [[0, text.length]];
  let bestLong = text.length;
  const choose = (from: number, left: number, cuts: number[]) => {
    if (left === 0) {
      const edges = [-1, ...cuts, text.length];
      const spans = edges
        .slice(1)
        .map((e, j) => [(edges[j] ?? 0) + 1, e] as [number, number]);
      const long = Math.max(...spans.map(([a, b]) => b - a));
      if (long < bestLong) {
        bestLong = long;
        best = spans;
      }
      return;
    }
    for (let j = from; j < spaces.length; j++)
      choose(j + 1, left - 1, [...cuts, spaces[j] ?? 0]);
  };
  choose(0, lines - 1, []);
  return best;
}

/**
 * The plaque's line in three lines of pixel rows, cut out of the whole
 * line's `textRows` at its spaces: glyph `i` covers columns `4i` to `4i +
 * 2`, so the line from character `a` to `b` is columns `4a` to `4b - 2`.
 */
export const PLAQUE_LINES: readonly (readonly string[])[] = (() => {
  const rows = textRows(MARKS.reactorPlaque);
  return lineBreaks(MARKS.reactorPlaque, 3).map(([a, b]) =>
    rows.map((r) => r.slice(4 * a, 4 * b - 1)),
  );
})();

const reactorCase: CurioRecipe = ({ k, s }) => {
  const { hw, hd, top } = curioHalf("reactor-case", 0);
  const casing = s.tinted(CASING);
  const casingTop = s.tinted(CASING_TOP);
  // The plinth and the slab the glass stands on.
  k.bevelBox(-hw, hw, -hd, PLAQUE_BACK, 0, PLINTH_TOP, 0.003, casing);
  k.box(
    -GLASS_HALF,
    GLASS_HALF,
    -GLASS_HALF,
    GLASS_HALF,
    PLINTH_TOP,
    SLAB_TOP,
    casingTop,
  );
  // The plaque and its line, centred line by line.
  k.box(
    -PLAQUE_HW,
    PLAQUE_HW,
    PLAQUE_BACK,
    PLAQUE_FACE,
    PLAQUE_H0,
    PLAQUE_H1,
    s.tinted(PLAQUE),
  );
  const letters = s.tinted(PLAQUE_TEXT);
  const block =
    (PLAQUE_LINES.length * 5 + (PLAQUE_LINES.length - 1) * LINE_GAP) *
    PLAQUE_PX;
  const textTop = (PLAQUE_H0 + PLAQUE_H1) / 2 + block / 2;
  PLAQUE_LINES.forEach((rows, i) =>
    markAt(
      k,
      rows,
      0,
      textTop - i * (5 + LINE_GAP) * PLAQUE_PX,
      PLAQUE_PX,
      PLAQUE_FACE + MARK_PROUD,
      letters,
    ),
  );
  // The post, up into the reactor's housing.
  k.cylinder(0, 0, SLAB_TOP, REACTOR_H - HOUSING_R + 0.006, 0.012, 12, casing);
  // The reactor, upright and facing +d: a deep round housing, the ring
  // on its face, the segments and the core.
  k.extrude(
    discOutline(0, REACTOR_H, HOUSING_R, 16),
    HOUSING_BACK,
    0.004,
    casingTop,
  );
  k.ring(
    0,
    0,
    REACTOR_H,
    RING_R,
    RING_TUBE,
    6,
    16,
    s.tinted(REACTOR_RING),
    "inward",
  );
  const segments = 10;
  const step = (2 * Math.PI) / segments;
  for (let i = 0; i < segments; i++) {
    const t0 = i * step + step * 0.12;
    const t1 = (i + 1) * step - step * 0.12;
    const at = (r: number, t: number) =>
      [r * Math.cos(t), REACTOR_H + r * Math.sin(t)] as const;
    k.extrude(
      [at(0.0225, t0), at(0.0305, t0), at(0.0305, t1), at(0.0225, t1)],
      0.004,
      0.01,
      s.blink(SEGMENT, 1),
    );
  }
  k.extrude(
    discOutline(0, REACTOR_H, 0.018, 12),
    0.004,
    0.012,
    s.blink(CORE, 0),
  );
  // The glass box: twelve thin pale bars on its edges.
  const glass = s.tinted(GLASS_EDGE);
  const g = GLASS_HALF;
  for (const a of [-g, g - BAR])
    for (const d of [-g, g - BAR])
      k.box(a, a + BAR, d, d + BAR, SLAB_TOP, top, glass);
  for (const h of [SLAB_TOP, top - BAR])
    for (const d of [-g, g - BAR]) {
      k.box(-g + BAR, g - BAR, d, d + BAR, h, h + BAR, glass);
      k.box(d, d + BAR, -g + BAR, g - BAR, h, h + BAR, glass);
    }
};

/** The finds' recipes. */
export const FIND_RECIPES = {
  "treasure-radar": treasureRadar,
  "capsule-case": capsuleCase,
  "reactor-case": reactorCase,
} satisfies Record<
  Extract<CurioKind, "treasure-radar" | "capsule-case" | "reactor-case">,
  CurioRecipe
>;
