/**
 * The arcade heroes' recipes: the arcade cabinet and the recruitment
 * cabinet. What they share is the cabinet: a tall body extruded from one
 * side profile, side panels of the same profile with art standing on their
 * outer faces, a marquee, a control deck with a stick and buttons, and an
 * attract screen that swaps between its game's title card and its demo
 * (H14). A cabinet's variant is its game (`ARCADE_GAMES`).
 *
 * How the parts are built:
 * - The body is `profileAlong` of the side profile in three pieces: two
 *   thin outer strips that keep the sloped screen face, and a middle
 *   piece whose profile stands vertical where the screen is, so the screen
 *   is a flat upright face set into the slope.
 * - The screen is layered on that face one `DECAL_LIFT` at a time: a dark
 *   bezel, a near-black glass, then the pixels. Titles and demos are
 *   block pixels (`pixels.ts`): one flat quad per run, emissive through
 *   the blink bank. The title card is blink groups 0 to 3 (one per column
 *   quarter of its lit width) in the upper band, the demo groups 4 to 7 in
 *   the lower band, so the swap bank shows one while the other dims, and
 *   no quad of one ever shares a place with a quad of the other.
 * - Side art is thin extruded shapes on the side panels' outer faces,
 *   built in a frame turned to face out of each side (`sideShape`).
 * - Colours are each game's palette (`ARCADE_GAMES`), the recruitment
 *   cabinet's named tints below and the look's `dark` and `metal` for the
 *   controls, so the cabinets keep their own colours in every look, as a
 *   real cabinet would.
 * - Nothing reaches past the footprint's front (`d1`): the use point in
 *   front of the control deck stays clear.
 */

import type { HeroKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import {
  profileAlong,
  shade,
  sideways,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import { heroHalf, type HeroRecipe } from "./common";
import { pixelPanel, textRows } from "./pixels";

/** A point of a side profile or a side shape: depth out from the wall, height. */
type DH = readonly [d: number, h: number];

/**
 * One game of the arcade cabinet (H14; every title and colour is ours):
 * its title in the block-pixel font, the cabinet's body colour, its side
 * panels' colour and its accent (marquee, buttons, title card), and its
 * demo picture for the attract screen's lower band, one row per string
 * from the top, `.` dark, `a` the accent, `s` the side colour and `w`
 * white.
 */
export interface ArcadeGame {
  title: string;
  body: Rgb;
  side: Rgb;
  accent: Rgb;
  demo: readonly string[];
}

/** TILEFALL's body: a dark blue. */
const TILE_NAVY: Rgb = [0.05, 0.07, 0.22];

/** TILEFALL's sides and resting blocks: cyan. */
const TILE_CYAN: Rgb = [0.1, 0.78, 0.88];

/** TILEFALL's accent and falling blocks: magenta. */
const TILE_MAGENTA: Rgb = [0.9, 0.14, 0.68];

/** ROCK RAIN's body: charcoal. */
const ROCK_CHARCOAL: Rgb = [0.13, 0.13, 0.14];

/** ROCK RAIN's sides and rocks: amber. */
const ROCK_AMBER: Rgb = [1.0, 0.6, 0.1];

/** ROCK RAIN's accent, ship and shots: a bright white. */
const ROCK_WHITE: Rgb = [0.95, 0.95, 0.92];

/** MAZE HUNT's body: near black. */
const MAZE_BLACK: Rgb = [0.03, 0.03, 0.04];

/** MAZE HUNT's sides and maze walls: a strong blue. */
const MAZE_BLUE: Rgb = [0.12, 0.24, 0.95];

/** MAZE HUNT's accent and its round chaser: yellow. */
const MAZE_YELLOW: Rgb = [1.0, 0.88, 0.1];

/** A demo's `w` pixels: a cool white, the well walls and the dots. */
const PIXEL_WHITE: Rgb = [0.82, 0.85, 0.9];

/** A screen's glass between the pixels: all but black. */
const SCREEN_BLACK: Rgb = [0.015, 0.02, 0.025];

/** The title on a lit marquee: dark ink over the glow. */
const MARQUEE_INK: Rgb = [0.04, 0.03, 0.05];

/** The coin slots' steady light: orange. */
const COIN_ORANGE: Rgb = [1.0, 0.5, 0.08];

/** The recruitment cabinet's body: a blue-black. */
const RECRUIT_BLACK: Rgb = [0.05, 0.05, 0.08];

/** The recruitment cabinet's glowing side panels: a deep blue-violet. */
const SIDE_VIOLET: Rgb = [0.3, 0.16, 0.85];

/** The recruitment cabinet's plain marquee: a pale violet glow. */
const MARQUEE_VIOLET: Rgb = [0.62, 0.55, 1.0];

/** A star on the side panels and the marquee: a cold white. */
const STAR_WHITE: Rgb = [0.95, 0.96, 1.0];

/** The fighter silhouette on the side panels: dark ink. */
const FIGHTER_INK: Rgb = [0.03, 0.02, 0.06];

/** The recruitment cabinet's fire buttons: red. */
const FIRE_RED: Rgb = [0.9, 0.08, 0.06];

/** The recruitment demo's grid: a vector cyan. */
const GRID_CYAN: Rgb = [0.2, 0.85, 1.0];

/** The recruitment demo's trench walls: a light violet. */
const TRENCH_VIOLET: Rgb = [0.7, 0.45, 1.0];

/** The recruitment title card: gold. */
const TITLE_GOLD: Rgb = [1.0, 0.78, 0.2];

/**
 * The arcade cabinet's three games, one per variant, each its own
 * palette and demo:
 * - 0 `TILEFALL`: a well (two walls and a floor) with stacked block pieces
 *   and one piece falling above them.
 * - 1 `ROCK RAIN`: drifting rock outlines, a small triangle ship and a
 *   dotted line of shots.
 * - 2 `MAZE HUNT`: blue maze walls, a round yellow chaser with its mouth
 *   open and a row of dots.
 */
export const ARCADE_GAMES: readonly ArcadeGame[] = [
  {
    title: "TILEFALL",
    body: TILE_NAVY,
    side: TILE_CYAN,
    accent: TILE_MAGENTA,
    demo: [
      ".......w...aaa....w.......",
      ".......w....a.....w.......",
      ".......w..........w.......",
      ".......w..........w.......",
      ".......w..........w.......",
      ".......wss......aaw.......",
      ".......wssaa..ssaaw.......",
      ".......wsaaassss.aw.......",
      ".......wwwwwwwwwwww.......",
    ],
  },
  {
    title: "ROCK RAIN",
    body: ROCK_CHARCOAL,
    side: ROCK_AMBER,
    accent: ROCK_WHITE,
    demo: [
      "..sss..............sss....",
      ".s...s.....a......s...s...",
      ".s...s.............sss....",
      "..sss......a..............",
      "..........................",
      ".......ss..a..............",
      ".......ss.................",
      "...........a..............",
      "..........aaa.............",
    ],
  },
  {
    title: "MAZE HUNT",
    body: MAZE_BLACK,
    side: MAZE_BLUE,
    accent: MAZE_YELLOW,
    demo: [
      "ssssssssssssssssssssssssss",
      "s...........ss...........s",
      "s.ssss.ssss.ss.ssss.ssss.s",
      "s...........aaa..........s",
      "s.w.w.w.w..aa...w.w.w.w..s",
      "s...........aaa..........s",
      "s.ssss.ssss.ss.ssss.ssss.s",
      "s...........ss...........s",
      "ssssssssssssssssssssssssss",
    ],
  },
];

/** The recruitment cabinet's on-screen title: our own. */
export const RECRUIT_TITLE = "VOID WING";

/**
 * The recruitment cabinet's demo, in the same letters as a game's (`a`
 * the grid, `s` the trench walls, `w` the fighter): a wireframe fighter
 * seen from behind, above a perspective grid whose lines close in towards
 * the horizon between two slanted trench walls.
 */
const RECRUIT_DEMO: readonly string[] = [
  "................w................",
  "...............w.w...............",
  ".............ww...ww.............",
  "..........www...w...www..........",
  "..........wwwww.w.wwwww..........",
  ".................................",
  "..........s.....a.....s..........",
  "........saaaaaaaaaaaaaaas........",
  "......s.........a.........s......",
  "....saaaaaaaaaaaaaaaaaaaaaaas....",
  "..s.............a.............s..",
  "saaaaaaaaaaaaaaaaaaaaaaaaaaaaaaas",
];

/** The frame every recipe builds in. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/**
 * Where a pixel picture of `rows` lands when fitted into the box `a0..a1`
 * by `h0..h1`: square pixels as large as both extents allow, the picture
 * centred in the box. Returns the pixel size, the left edge and the top.
 */
function fit(
  rows: readonly string[],
  a0: number,
  a1: number,
  h0: number,
  h1: number,
): { px: number; left: number; top: number } {
  const cols = rows[0]?.length ?? 0;
  const px = Math.min((a1 - a0) / cols, (h1 - h0) / rows.length);
  return {
    px,
    left: (a0 + a1) / 2 - (cols * px) / 2,
    top: (h0 + h1) / 2 + (rows.length * px) / 2,
  };
}

/**
 * Draws a pixel picture fitted into the box `a0..a1` by `h0..h1` at depth
 * `d` as blinking pixels: the columns from its first to its last lit one
 * are cut into four quarters, and quarter `q` blinks in group `group0 +
 * q`. `tintOf` gives a character's colour, or `null` for a dark one.
 */
function blinkPicture(
  k: Kit,
  s: Surfaces,
  rows: readonly string[],
  box: readonly [a0: number, a1: number, h0: number, h1: number],
  d: number,
  group0: number,
  tintOf: (ch: string) => Rgb | null,
): void {
  const { px, left, top } = fit(rows, ...box);
  const litCols = rows.flatMap((r) =>
    [...r].flatMap((ch, i) => (tintOf(ch) === null ? [] : [i])),
  );
  const lo = Math.min(...litCols);
  const span = Math.max(...litCols) + 1 - lo;
  for (let q = 0; q < 4; q++) {
    const c0 = lo + Math.round((q * span) / 4);
    const c1 = lo + Math.round(((q + 1) * span) / 4);
    pixelPanel(
      k,
      rows.map((r) => r.slice(c0, c1)),
      left + c0 * px,
      top,
      px,
      d,
      (ch) => {
        const t = tintOf(ch);
        return t === null ? null : s.blink(t, group0 + q);
      },
    );
  }
}

/**
 * A thin shape standing on a cabinet's side: `outline` in the side view's
 * `(d, h)`, extruded from `out0` to `out1` out from the middle on side
 * `side` (`1` the `+a` side, `-1` the `-a` side). Built in a frame turned
 * to look out of that side, so the same outline reads the same way on
 * both faces.
 */
function sideShape(
  kitAt: KitAt,
  side: 1 | -1,
  outline: readonly DH[],
  out0: number,
  out1: number,
  s: Surface,
): void {
  const f = side > 0 ? sideways(ORIGIN) : yawed(ORIGIN, 0, 0, Math.PI / 2);
  kitAt(f).extrude(
    outline.map(([d, h]) => [-side * d, h] as const),
    out0,
    out1,
    s,
  );
}

/** A rectangle `d0..d1` by `h0..h1` in the side view: an outline for `sideShape`. */
const rect = (d0: number, d1: number, h0: number, h1: number): DH[] => [
  [d0, h0],
  [d1, h0],
  [d1, h1],
  [d0, h1],
];

/**
 * An outline of `n` corners around `(d, h)` at radius `r`, each corner's
 * radius scaled by `wobble` (cycled), starting at `turn` radians: a rock
 * when the wobble is uneven, a disc when it is all 1.
 */
function blob(
  d: number,
  h: number,
  r: number,
  n: number,
  wobble: readonly number[],
  turn = 0,
): DH[] {
  return Array.from({ length: n }, (_, i) => {
    const t = turn + (2 * Math.PI * i) / n;
    const w = wobble[i % wobble.length] ?? 1;
    return [d + r * w * Math.cos(t), h + r * w * Math.sin(t)] as const;
  });
}

/** The arcade cabinet's side profile (the plan's, verbatim): deck front, deck, screen slope, marquee, top. */
const ARCADE_PROFILE: readonly DH[] = [
  [0, 0],
  [0.75, 0],
  [0.75, 0.86],
  [0.55, 0.98],
  [0.47, 1.06],
  [0.42, 1.55],
  [0.56, 1.6],
  [0.56, 1.9],
  [0, 1.95],
];

/** The arcade screen: the upright face's depth and its bottom and top. */
const ARCADE_SCREEN = { d: 0.45, h0: 1.08, h1: 1.5 } as const;

/**
 * The arcade profile of the screen's width: the same outline, but the
 * slope stands upright at `ARCADE_SCREEN.d` from its bottom to its top, so
 * the screen is a vertical face set into the slope.
 */
const ARCADE_SCREEN_PROFILE: readonly DH[] = [
  [0, 0],
  [0.75, 0],
  [0.75, 0.86],
  [0.55, 0.98],
  [0.47, 1.06],
  [ARCADE_SCREEN.d, ARCADE_SCREEN.h0],
  [ARCADE_SCREEN.d, ARCADE_SCREEN.h1],
  [0.42, 1.55],
  [0.56, 1.6],
  [0.56, 1.9],
  [0, 1.95],
];

/** The arcade cabinet's control deck: its height at depth `d`, from 0.86 at the front (0.75) to 0.98 at 0.55. */
const arcadeDeck = (d: number) => 0.86 + ((0.75 - d) * 0.12) / 0.2;

/**
 * Each game's side art in the side view, drawn in the game's palette:
 * - TILEFALL: stacked blocks in cyan and magenta (runs of cells, each
 *   run a block bar with a small gap round it).
 * - ROCK RAIN: three rock outlines, a white rock with an amber one a
 *   little smaller standing on it, so only the white rim shows.
 * - MAZE HUNT: maze lines in the body's black, with the yellow chaser in
 *   the middle.
 * Each entry lists `[outline, surface]` pairs; the art stands between the
 * side panel's face and the footprint's edge.
 */
function sideArt(
  game: ArcadeGame,
  variant: number,
  s: Surfaces,
): { outline: DH[]; surface: Surface; lift: number }[] {
  const accent = s.tinted(game.accent);
  const deepSide = s.tinted(shade(game.side, 0.45));
  if (variant === 0) {
    const cell = 0.085;
    const gap = 0.006;
    const rows = [
      "..a...",
      ".aaa..",
      "......",
      "s...ss",
      "ss.aas",
      "saaa.s",
      "sssaas",
    ];
    return rows.flatMap((row, r) => {
      const h1 = 0.22 + (rows.length - r) * cell;
      const out: { outline: DH[]; surface: Surface; lift: number }[] = [];
      let c = 0;
      while (c < row.length) {
        const ch = row[c] ?? ".";
        let e = c + 1;
        while (e < row.length && row[e] === ch) e++;
        if (ch !== ".")
          out.push({
            outline: rect(
              0.08 + c * cell + gap,
              0.08 + e * cell - gap,
              h1 - cell + gap,
              h1 - gap,
            ),
            surface: ch === "a" ? accent : deepSide,
            lift: 0,
          });
        c = e;
      }
      return out;
    });
  }
  if (variant === 1) {
    const rock = [1, 0.8, 1.05, 0.75, 0.95, 0.85, 1.1];
    const rim = s.tinted(game.accent);
    const inner = s.tinted(game.side);
    return (
      [
        [0.5, 0.55, 0.16],
        [0.22, 0.95, 0.1],
        [0.27, 1.32, 0.08],
      ] as const
    ).flatMap(([d, h, r]) => [
      { outline: blob(d, h, r, 7, rock, 0.3), surface: rim, lift: 0 },
      { outline: blob(d, h, r - 0.02, 7, rock, 0.3), surface: inner, lift: 1 },
    ]);
  }
  // The maze: lines on a grid of `step`, origin at (d 0.1, h 0.2).
  const step = 0.05;
  const w = 0.012;
  const lines: readonly (readonly [number, number, number, number])[] = [
    [0, 0, 11, 0],
    [0, 11, 11, 11],
    [0, 0, 0, 4],
    [0, 7, 0, 11],
    [11, 0, 11, 4],
    [11, 7, 11, 11],
    [2, 2, 4, 2],
    [7, 2, 9, 2],
    [2, 2, 2, 4],
    [9, 2, 9, 4],
    [4, 4, 7, 4],
    [5.5, 0, 5.5, 2],
    [2, 7, 2, 9],
    [9, 7, 9, 9],
    [2, 9, 4.5, 9],
    [6.5, 9, 9, 9],
    [4, 7, 7, 7],
    [5.5, 9, 5.5, 11],
  ];
  const ink = s.tinted(game.body);
  return [
    ...lines.map(([x0, y0, x1, y1]) => ({
      outline: rect(
        0.1 + x0 * step - w,
        0.1 + x1 * step + w,
        0.2 + y0 * step - w,
        0.2 + y1 * step + w,
      ),
      surface: ink,
      lift: 0,
    })),
    {
      outline: blob(0.1 + 5.5 * step, 0.2 + 5.5 * step, 0.045, 10, [1]),
      surface: accent,
      lift: 0,
    },
  ];
}

/**
 * The arcade cabinet, backed against its wall, one game per variant
 * (`ARCADE_GAMES`):
 * - The body in the game's body colour, `profileAlong` of the side
 *   profile (`ARCADE_PROFILE`) over `a` -0.36 to 0.36: an upright front
 *   to the control deck (h 0.86), the deck sloping back up, the screen
 *   slope, the marquee's upright face (d 0.56, h 1.6 to 1.9) and the top.
 *   Over the screen's width the slope stands upright at d 0.45
 *   (`ARCADE_SCREEN_PROFILE`).
 * - Side panels of the same profile out to 5 mm inside the footprint's
 *   edge, in the game's side colour, the game's side art on their outer
 *   faces (`sideArt`).
 * - The screen, upright at d 0.45 from h 1.08 to 1.5: a dark bezel, a
 *   black glass, the title card (`textRows(title)`, groups 0 to 3) in the
 *   band h 1.33 to 1.47 and the demo (groups 4 to 7) in h 1.10 to 1.30,
 *   in the swap bank.
 * - The marquee (h 1.62 to 1.86, on the upright face at d 0.56): a steady
 *   `s.signal` glow in the accent colour with the title in dark lit
 *   pixels on it.
 * - The control deck: a dark plate, a joystick (a metal shaft and a ball
 *   in the accent colour) and three accent buttons; under it a coin door
 *   (a dark box at h 0.45) with two small steady orange slots.
 */
const arcadeCabinet: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const game = ARCADE_GAMES[variant];
  if (!game) throw new Error(`arcade: no game for variant ${String(variant)}`);
  const body = s.tinted(game.body);
  const inner = hw - 0.04;
  const panelOut = hw - 0.005;
  const screenHalf = 0.33;
  // The body: two strips keeping the slope, the middle with the screen face.
  for (const [a0, a1] of [
    [-inner, -screenHalf],
    [screenHalf, inner],
  ] as const)
    profileAlong(kitAt, ORIGIN, ARCADE_PROFILE, a0, a1, body);
  profileAlong(
    kitAt,
    ORIGIN,
    ARCADE_SCREEN_PROFILE,
    -screenHalf,
    screenHalf,
    body,
  );
  // The side panels and their art.
  const sideS = s.tinted(game.side);
  profileAlong(kitAt, ORIGIN, ARCADE_PROFILE, -panelOut, -inner, sideS);
  profileAlong(kitAt, ORIGIN, ARCADE_PROFILE, inner, panelOut, sideS);
  const art = sideArt(game, variant, s);
  for (const side of [1, -1] as const)
    for (const { outline, surface, lift } of art) {
      const out0 = panelOut + lift * 0.0035;
      sideShape(kitAt, side, outline, out0, lift ? hw : out0 + 0.0035, surface);
    }
  // The screen.
  const { d: sd, h0: sh0, h1: sh1 } = ARCADE_SCREEN;
  k.panel(-screenHalf, screenHalf, sd + DECAL_LIFT, sh0, sh1, s.dark);
  k.panel(
    -0.3,
    0.3,
    sd + 2 * DECAL_LIFT,
    sh0 + 0.015,
    sh1 - 0.015,
    s.tinted(SCREEN_BLACK),
  );
  const pixD = sd + 3 * DECAL_LIFT;
  blinkPicture(
    k,
    s,
    textRows(game.title),
    [-0.28, 0.28, 1.33, 1.47],
    pixD,
    0,
    (ch) => (ch === "#" ? game.accent : null),
  );
  const demoTint: Record<string, Rgb> = {
    a: game.accent,
    s: game.side,
    w: PIXEL_WHITE,
  };
  blinkPicture(
    k,
    s,
    game.demo,
    [-0.28, 0.28, 1.1, 1.3],
    pixD,
    4,
    (ch) => demoTint[ch] ?? null,
  );
  // The marquee.
  const md = 0.56;
  k.panel(-0.34, 0.34, md + DECAL_LIFT, 1.62, 1.86, s.signal(game.accent));
  const title = textRows(game.title);
  const m = fit(title, -0.31, 0.31, 1.66, 1.82);
  const ink = s.tinted(MARQUEE_INK);
  pixelPanel(k, title, m.left, m.top, m.px, md + 2 * DECAL_LIFT, (ch) =>
    ch === "#" ? ink : null,
  );
  // The control deck.
  const [p0, p1] = [0.73, 0.57];
  profileAlong(
    kitAt,
    ORIGIN,
    [
      [p0, arcadeDeck(p0) - 0.005],
      [p1, arcadeDeck(p1) - 0.005],
      [p1, arcadeDeck(p1) + 0.008],
      [p0, arcadeDeck(p0) + 0.008],
    ],
    -0.3,
    0.3,
    s.dark,
  );
  const [ja, jd] = [-0.15, 0.66];
  const jh = arcadeDeck(jd);
  const ballH = jh + 0.1;
  k.cylinder(ja, jd, jh, ballH + 0.01, 0.011, 6, s.metal);
  k.lathe(
    ja,
    jd,
    [
      [0, ballH],
      [0.02, ballH + 0.006],
      [0.028, ballH + 0.025],
      [0.02, ballH + 0.044],
      [0, ballH + 0.05],
    ],
    8,
    s.tinted(game.accent),
  );
  for (const a of [0.04, 0.12, 0.2])
    k.cylinder(
      a,
      jd,
      jh - 0.01,
      arcadeDeck(jd) + 0.02,
      0.024,
      8,
      s.tinted(game.accent),
    );
  // The coin door with its two slots.
  const front = 0.75;
  k.box(-0.12, 0.12, front, front + 0.02, 0.3, 0.6, s.dark);
  for (const a of [-0.05, 0.05])
    k.box(
      a - 0.012,
      a + 0.012,
      front + 0.02,
      front + 0.03,
      0.47,
      0.53,
      s.signal(COIN_ORANGE),
    );
};

/** The recruitment cabinet's side profile (the plan's, verbatim): deep deck, screen slope, hood, top. */
const RECRUIT_PROFILE: readonly DH[] = [
  [0, 0],
  [1.15, 0],
  [1.15, 0.82],
  [0.78, 1.02],
  [0.72, 1.2],
  [0.56, 1.72],
  [0.72, 1.84],
  [0.64, 2.0],
  [0, 2.0],
];

/** The recruitment screen: the upright face's depth and its bottom and top. */
const RECRUIT_SCREEN = { d: 0.62, h0: 1.25, h1: 1.7 } as const;

/** The recruitment profile of the screen's width: the slope stands upright at the screen. */
const RECRUIT_SCREEN_PROFILE: readonly DH[] = [
  [0, 0],
  [1.15, 0],
  [1.15, 0.82],
  [0.78, 1.02],
  [0.72, 1.2],
  [RECRUIT_SCREEN.d, RECRUIT_SCREEN.h0],
  [RECRUIT_SCREEN.d, RECRUIT_SCREEN.h1],
  [0.56, 1.72],
  [0.72, 1.84],
  [0.64, 2.0],
  [0, 2.0],
];

/** The recruitment control deck: its height at depth `d`, from 0.82 at the front (1.15) to 1.02 at 0.78. */
const recruitDeck = (d: number) => 0.82 + ((1.15 - d) * 0.2) / 0.37;

/** The stars on the recruitment side panels, in the side view. */
const SIDE_STARS: readonly DH[] = [
  [0.15, 0.3],
  [0.4, 0.55],
  [0.8, 0.25],
  [1.0, 0.6],
  [0.6, 0.78],
  [0.25, 0.85],
  [0.95, 0.42],
  [0.15, 1.4],
  [0.3, 1.75],
  [0.2, 1.9],
  [0.5, 1.62],
  [0.9, 0.9],
];

/** The fighter silhouette on the recruitment side panels: an arrowhead with two swept wings, nose up. */
const FIGHTER: readonly DH[] = [
  [0.45, 1.5],
  [0.53, 1.2],
  [0.72, 1.04],
  [0.52, 1.12],
  [0.45, 1.08],
  [0.38, 1.12],
  [0.18, 1.04],
  [0.37, 1.2],
];

/**
 * The recruitment cabinet, backed against its wall, wider and deeper than
 * the arcade cabinet, with a long sloped side:
 * - The body in blue-black, `profileAlong` of `RECRUIT_PROFILE` over `a`
 *   -0.46 to 0.46: a deep control deck rising from h 0.82 to 1.02, the
 *   screen slope, a hood over the screen and the top at 2.0. Over the
 *   screen's width the slope stands upright at d 0.62.
 * - The side panels of the same profile glow a steady deep blue-violet
 *   (`s.signal`), with brighter star dots (small signal squares set into
 *   the panel from inside, so they touch the body) and a fighter
 *   silhouette (a flat dark arrowhead with two swept wings) standing on
 *   their outer faces.
 * - The screen, upright at d 0.62 from h 1.25 to 1.7: a dark bezel, a
 *   black glass, `VOID WING` in the upper band (groups 0 to 3) and the
 *   demo (`RECRUIT_DEMO`, groups 4 to 7) in the lower band, in the swap
 *   bank.
 * - A plain glowing marquee on the hood's face with a few stars on it and
 *   no text.
 * - The control deck: a dark plate, a flight stick (a column on a boot
 *   with a T grip) and two red fire buttons.
 */
const recruitCabinet: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const body = s.tinted(RECRUIT_BLACK);
  const inner = hw - 0.04;
  const panelOut = hw - 0.005;
  const screenHalf = 0.38;
  for (const [a0, a1] of [
    [-inner, -screenHalf],
    [screenHalf, inner],
  ] as const)
    profileAlong(kitAt, ORIGIN, RECRUIT_PROFILE, a0, a1, body);
  profileAlong(
    kitAt,
    ORIGIN,
    RECRUIT_SCREEN_PROFILE,
    -screenHalf,
    screenHalf,
    body,
  );
  // The glowing side panels, their stars and the fighter.
  const glow = s.signal(SIDE_VIOLET);
  profileAlong(kitAt, ORIGIN, RECRUIT_PROFILE, -panelOut, -inner, glow);
  profileAlong(kitAt, ORIGIN, RECRUIT_PROFILE, inner, panelOut, glow);
  const star = s.signal(STAR_WHITE);
  const ink = s.tinted(FIGHTER_INK);
  const r = 0.009;
  for (const side of [1, -1] as const) {
    for (const [d, h] of SIDE_STARS)
      sideShape(
        kitAt,
        side,
        rect(d - r, d + r, h - r, h + r),
        inner + 0.015,
        hw,
        star,
      );
    sideShape(kitAt, side, FIGHTER, panelOut, hw, ink);
  }
  // The screen.
  const { d: sd, h0: sh0, h1: sh1 } = RECRUIT_SCREEN;
  k.panel(-screenHalf, screenHalf, sd + DECAL_LIFT, sh0, sh1, s.dark);
  k.panel(
    -0.35,
    0.35,
    sd + 2 * DECAL_LIFT,
    sh0 + 0.01,
    sh1 - 0.01,
    s.tinted(SCREEN_BLACK),
  );
  const pixD = sd + 3 * DECAL_LIFT;
  blinkPicture(
    k,
    s,
    textRows(RECRUIT_TITLE),
    [-0.33, 0.33, 1.56, 1.67],
    pixD,
    0,
    (ch) => (ch === "#" ? TITLE_GOLD : null),
  );
  const demoTint: Record<string, Rgb> = {
    a: GRID_CYAN,
    s: TRENCH_VIOLET,
    w: PIXEL_WHITE,
  };
  blinkPicture(
    k,
    s,
    RECRUIT_DEMO,
    [-0.33, 0.33, 1.27, 1.53],
    pixD,
    4,
    (ch) => demoTint[ch] ?? null,
  );
  // The marquee on the hood's face, from (0.72, 1.84) up to (0.64, 2.0).
  const [q0, q1] = [RECRUIT_PROFILE[6], RECRUIT_PROFILE[7]];
  if (!q0 || !q1) throw new Error("recruit: the hood is missing");
  const len = Math.hypot(q1[0] - q0[0], q1[1] - q0[1]);
  const normal: DH = [(q1[1] - q0[1]) / len, (q0[0] - q1[0]) / len];
  const onHood = (u: number, n: number): DH => [
    q0[0] + u * (q1[0] - q0[0]) + n * normal[0],
    q0[1] + u * (q1[1] - q0[1]) + n * normal[1],
  ];
  const thick = 0.006;
  profileAlong(
    kitAt,
    ORIGIN,
    [
      onHood(0.08, 0),
      onHood(0.92, 0),
      onHood(0.92, thick),
      onHood(0.08, thick),
    ],
    -screenHalf,
    screenHalf,
    s.signal(MARQUEE_VIOLET),
  );
  const e = 0.035;
  for (const [a, u] of [
    [-0.3, 0.3],
    [-0.12, 0.7],
    [0.04, 0.4],
    [0.2, 0.75],
    [0.32, 0.35],
  ] as const)
    profileAlong(
      kitAt,
      ORIGIN,
      [
        onHood(u - e, -0.01),
        onHood(u + e, -0.01),
        onHood(u + e, thick + 0.003),
        onHood(u - e, thick + 0.003),
      ],
      a - 0.006,
      a + 0.006,
      star,
    );
  // The control deck.
  const [p0, p1] = [1.12, 0.82];
  profileAlong(
    kitAt,
    ORIGIN,
    [
      [p0, recruitDeck(p0) - 0.005],
      [p1, recruitDeck(p1) - 0.005],
      [p1, recruitDeck(p1) + 0.008],
      [p0, recruitDeck(p0) + 0.008],
    ],
    -0.4,
    0.4,
    s.dark,
  );
  const [fa, fd] = [0, 0.97];
  const fh = recruitDeck(fd);
  k.cylinder(fa, fd, fh - 0.01, fh + 0.04, 0.05, 10, s.dark);
  k.cylinder(fa, fd, fh + 0.04, fh + 0.2, 0.02, 8, s.metal);
  k.cylinderAlong(fa - 0.1, fa + 0.1, fd, fh + 0.21, 0.022, 8, s.dark);
  const bd = 1.0;
  for (const a of [-0.26, 0.26])
    k.cylinder(
      a,
      bd,
      recruitDeck(bd) - 0.01,
      recruitDeck(bd) + 0.02,
      0.032,
      10,
      s.tinted(FIRE_RED),
    );
};

/** The arcade kinds' recipes. */
export const ARCADE_RECIPES = {
  "arcade-cabinet": arcadeCabinet,
  "recruit-cabinet": recruitCabinet,
} satisfies Record<
  Extract<HeroKind, "arcade-cabinet" | "recruit-cabinet">,
  HeroRecipe
>;
