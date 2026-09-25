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
  discOutline,
  profileAlong,
  shade,
  sideways,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import { heroHalf, type HeroRecipe } from "./common";
import { pixelPanel, runsOf, textRows } from "./pixels";

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
export const ARCADE_GAMES = [
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
] as const satisfies readonly ArcadeGame[];

/** A game's title, the key its side art is held to (`SIDE_ART`). */
type GameTitle = (typeof ARCADE_GAMES)[number]["title"];

/** The recruitment cabinet's on-screen title: our own. */
export const RECRUIT_TITLE = "VOID WING";

/**
 * The recruitment cabinet's demo, in the same letters as a game's (`a`
 * the grid, `s` the trench walls, `w` the fighter): a wireframe fighter
 * seen from behind, above a perspective grid (cross lines, a centre line
 * and two diagonals running in to the vanishing point) between two slanted
 * trench walls. Exported for the test that holds its rows to one width.
 */
export const RECRUIT_DEMO: readonly string[] = [
  "................w................",
  "...............w.w...............",
  ".............ww...ww.............",
  "..........www...w...www..........",
  "..........wwwww.w.wwwww..........",
  ".................................",
  "..........s..a..a..a..s..........",
  "........saaaaaaaaaaaaaaas........",
  "......s....a....a....a....s......",
  "....saaaaaaaaaaaaaaaaaaaaaaas....",
  "..s......a......a......a......s..",
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
 * A rock's outline: `n` corners around `(d, h)` at radius `r`, each
 * corner's radius scaled by `wobble` (cycled), starting at `turn` radians,
 * so the rock is lumpy rather than round.
 */
function rockOutline(
  d: number,
  h: number,
  r: number,
  n: number,
  wobble: readonly number[],
  turn: number,
): DH[] {
  return Array.from({ length: n }, (_, i) => {
    const t = turn + (2 * Math.PI * i) / n;
    const w = wobble[i % wobble.length] ?? 1;
    return [d + r * w * Math.cos(t), h + r * w * Math.sin(t)] as const;
  });
}

/**
 * The face of a side profile from `p` to `q` as a map from `(u, n)` to the
 * side view: `u` runs from 0 at `p` to 1 at `q`, `n` is metres out of the
 * face (a profile runs counter-clockwise in the side view, so out is to
 * the right of `p` to `q`). What stands on a sloped face (a deck plate, a
 * marquee on a hood, a star on it) is built in these terms, so its layers
 * keep their spacing along the face's normal, not only in height.
 */
function onFace(p: DH, q: DH): (u: number, n: number) => DH {
  const len = Math.hypot(q[0] - p[0], q[1] - p[1]);
  const nd = (q[1] - p[1]) / len;
  const nh = (p[0] - q[0]) / len;
  return (u, n) => [
    p[0] + u * (q[0] - p[0]) + n * nd,
    p[1] + u * (q[1] - p[1]) + n * nh,
  ];
}

/** A profile's point `i`, failing loudly on an index it does not have. */
function at(profile: readonly DH[], i: number): DH {
  const p = profile[i];
  if (!p) throw new Error(`cabinet: the profile has no point ${String(i)}`);
  return p;
}

/**
 * A side profile with its screen stood upright: the points `[screen.d,
 * screen.h0]` and `[screen.d, screen.h1]` inserted after point
 * `afterIndex` (the slope's foot), so the slope between them becomes a
 * vertical face at the screen's depth and the rest of the outline stays
 * the profile's own.
 */
function withScreen(
  profile: readonly DH[],
  afterIndex: number,
  screen: { d: number; h0: number; h1: number },
): DH[] {
  return [
    ...profile.slice(0, afterIndex + 1),
    [screen.d, screen.h0],
    [screen.d, screen.h1],
    ...profile.slice(afterIndex + 1),
  ];
}

/**
 * What a cabinet's shared construction needs:
 * - `profile`: the side profile (the plan's, verbatim).
 * - `slopeFoot`: the index of the slope's foot, after which `withScreen`
 *   stands the screen; `deckFront`: the index of the deck's front edge,
 *   the deck running from it to the next point.
 * - `screen`: the upright screen face's depth and its bottom and top;
 *   `screenHalf` the half width of that upright piece and its bezel;
 *   `glassHalf` and `glassInset` the black glass inside the bezel.
 * - `bodyHalf`: the body's half width, where the side panels start;
 *   `sideOut`: where they end.
 * - `plateHalf`: the half width of the dark plate on the deck.
 */
interface CabinetSpec {
  profile: readonly DH[];
  slopeFoot: number;
  deckFront: number;
  screen: { d: number; h0: number; h1: number };
  screenHalf: number;
  glassHalf: number;
  glassInset: number;
  bodyHalf: number;
  sideOut: number;
  plateHalf: number;
}

/** The share of the deck's length its plate leaves bare at each end. */
const PLATE_MARGIN = 0.1;

/**
 * What `cabinet` hands back for the parts a recipe adds: the pixels'
 * depth on the screen (`pixD`, three decal layers out from its face), the
 * plate's top at `t` along the deck (0 at its front edge, 1 at its back)
 * and the deck's rise (height per metre of depth), which a round part on
 * the plate needs so that its top clears the slope all round.
 */
interface CabinetParts {
  pixD: number;
  deck: (t: number) => { d: number; h: number };
  rise: number;
}

/**
 * The construction both cabinets share:
 * - The body in `body`, `profileAlong` of the profile in three pieces:
 *   two strips from `screenHalf` out to `bodyHalf` keeping the sloped
 *   face, and the middle piece with the screen stood upright
 *   (`withScreen`).
 * - Side panels of the same profile from `bodyHalf` to `sideOut` in `side`.
 * - The screen stack on the upright face, one `DECAL_LIFT` per layer: a
 *   dark bezel over the whole face, the black glass, then (the recipe's)
 *   pixels.
 * - A dark plate on the deck, from `PLATE_MARGIN` to `1 - PLATE_MARGIN`
 *   along it, its top `DECAL_LIFT` out of the deck along the deck's
 *   normal.
 */
function cabinet(
  k: Kit,
  kitAt: KitAt,
  s: Surfaces,
  c: CabinetSpec,
  body: Surface,
  side: Surface,
): CabinetParts {
  const { profile, screen, screenHalf: w, bodyHalf, sideOut } = c;
  for (const [a0, a1] of [
    [-bodyHalf, -w],
    [w, bodyHalf],
  ] as const)
    profileAlong(kitAt, ORIGIN, profile, a0, a1, body);
  profileAlong(
    kitAt,
    ORIGIN,
    withScreen(profile, c.slopeFoot, screen),
    -w,
    w,
    body,
  );
  profileAlong(kitAt, ORIGIN, profile, -sideOut, -bodyHalf, side);
  profileAlong(kitAt, ORIGIN, profile, bodyHalf, sideOut, side);
  k.panel(-w, w, screen.d + DECAL_LIFT, screen.h0, screen.h1, s.dark);
  k.panel(
    -c.glassHalf,
    c.glassHalf,
    screen.d + 2 * DECAL_LIFT,
    screen.h0 + c.glassInset,
    screen.h1 - c.glassInset,
    s.tinted(SCREEN_BLACK),
  );
  const p = at(profile, c.deckFront);
  const q = at(profile, c.deckFront + 1);
  const deck = onFace(p, q);
  const [u0, u1] = [PLATE_MARGIN, 1 - PLATE_MARGIN];
  profileAlong(
    kitAt,
    ORIGIN,
    [
      deck(u0, -0.005),
      deck(u1, -0.005),
      deck(u1, DECAL_LIFT),
      deck(u0, DECAL_LIFT),
    ],
    -c.plateHalf,
    c.plateHalf,
    s.dark,
  );
  return {
    pixD: screen.d + 3 * DECAL_LIFT,
    deck: (t) => {
      const [d, h] = deck(t, DECAL_LIFT);
      return { d, h };
    },
    rise: Math.abs((q[1] - p[1]) / (q[0] - p[0])),
  };
}

/**
 * A round part standing on a cabinet's deck plate at `a` and `t` along
 * the deck: an upright cylinder of `radius` whose top stands `height`
 * above the plate's highest point under it and whose foot sinks below its
 * lowest, so it neither floats nor sinks into the slope.
 */
function onDeck(
  k: Kit,
  parts: CabinetParts,
  a: number,
  t: number,
  radius: number,
  height: number,
  sides: number,
  s: Surface,
): { d: number; top: number } {
  const { d, h } = parts.deck(t);
  const slack = radius * parts.rise;
  k.cylinder(a, d, h - slack - 0.01, h + slack + height, radius, sides, s);
  return { d, top: h + slack + height };
}

/** One piece of side art: its outline in the side view, its surface and its decal layer. */
interface ArtPiece {
  outline: DH[];
  surface: Surface;
  lift: 0 | 1;
}

/** A game's side art, drawn in its palette. */
type SideArt = (game: ArcadeGame, s: Surfaces) => ArtPiece[];

/**
 * TILEFALL's side art: stacked blocks in magenta and a deep cyan, one
 * block bar per run of cells (`runsOf`), each with a small gap round it.
 */
const blockArt: SideArt = (game, s) => {
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
  const accent = s.tinted(game.accent);
  const deepSide = s.tinted(shade(game.side, 0.45));
  return runsOf(rows)
    .filter((r) => r.ch !== ".")
    .map((r) => {
      const h1 = 0.22 + (rows.length - r.row) * cell;
      return {
        outline: rect(
          0.08 + r.col * cell + gap,
          0.08 + (r.col + r.len) * cell - gap,
          h1 - cell + gap,
          h1 - gap,
        ),
        surface: r.ch === "a" ? accent : deepSide,
        lift: 0,
      };
    });
};

/**
 * ROCK RAIN's side art: three rock outlines, each a white rock (layer 0)
 * with an amber one a little smaller on it (layer 1), so only the white
 * rim shows.
 */
const rockArt: SideArt = (game, s) => {
  const wobble = [1, 0.8, 1.05, 0.75, 0.95, 0.85, 1.1];
  const rim = s.tinted(game.accent);
  const middle = s.tinted(game.side);
  return (
    [
      [0.5, 0.55, 0.16],
      [0.22, 0.95, 0.1],
      [0.27, 1.32, 0.08],
    ] as const
  ).flatMap(([d, h, r]) => [
    { outline: rockOutline(d, h, r, 7, wobble, 0.3), surface: rim, lift: 0 },
    {
      outline: rockOutline(d, h, r - 0.02, 7, wobble, 0.3),
      surface: middle,
      lift: 1,
    },
  ]);
};

/**
 * MAZE HUNT's side art: maze lines in the body's black on a grid of 5 cm
 * from (d 0.1, h 0.2), the horizontal lines in layer 0 and the vertical
 * ones in layer 1 so a corner or a junction never stacks two faces at one
 * depth, and the yellow chaser in the middle.
 */
const mazeArt: SideArt = (game, s) => {
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
    ...lines.map(([x0, y0, x1, y1]): ArtPiece => ({
      outline: rect(
        0.1 + x0 * step - w,
        0.1 + x1 * step + w,
        0.2 + y0 * step - w,
        0.2 + y1 * step + w,
      ),
      surface: ink,
      lift: x0 === x1 ? 1 : 0,
    })),
    {
      outline: discOutline(0.1 + 5.5 * step, 0.2 + 5.5 * step, 0.045, 10),
      surface: s.tinted(game.accent),
      lift: 0,
    },
  ];
};

/** Each game's side art, keyed by its title so a reordered game list keeps art and palette together. */
const SIDE_ART = {
  TILEFALL: blockArt,
  "ROCK RAIN": rockArt,
  "MAZE HUNT": mazeArt,
} satisfies Record<GameTitle, SideArt>;

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

/**
 * The arcade cabinet, backed against its wall, one game per variant
 * (`ARCADE_GAMES`), built on `cabinet`:
 * - The body in the game's body colour over `a` -0.36 to 0.36: an upright
 *   front to the control deck (h 0.86), the deck sloping back up, the
 *   screen slope, the marquee's upright face (d 0.56, h 1.6 to 1.9) and the
 *   top. Over the screen's width (`a` +-0.33) the slope stands upright at
 *   d 0.45 from h 1.08 to 1.5.
 * - Side panels from `a` +-0.36 out to two `DECAL_LIFT` inside the
 *   footprint's edge, in the game's side colour, with its side art
 *   (`SIDE_ART`) in the last two decal layers, so the art ends on the
 *   footprint's edge rather than past it.
 * - The screen: the title card (`textRows(title)`, groups 0 to 3) in the
 *   band h 1.33 to 1.47 and the demo (groups 4 to 7) in h 1.10 to 1.30, in
 *   the swap bank.
 * - The marquee (h 1.62 to 1.86, on the upright face at d 0.56): a steady
 *   `s.signal` glow in the accent colour with the title in dark lit
 *   pixels on it.
 * - On the deck plate a joystick (a metal shaft and a ball in the accent
 *   colour) and three accent buttons; under the deck a coin door (a dark
 *   box at h 0.45) with two small steady orange slots.
 */
const arcadeCabinet: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const game = ARCADE_GAMES[variant];
  if (!game) throw new Error(`arcade: no game for variant ${String(variant)}`);
  const accent = s.tinted(game.accent);
  const sideOut = hw - 2 * DECAL_LIFT;
  const parts = cabinet(
    k,
    kitAt,
    s,
    {
      profile: ARCADE_PROFILE,
      slopeFoot: 4,
      deckFront: 2,
      screen: { d: 0.45, h0: 1.08, h1: 1.5 },
      screenHalf: 0.33,
      glassHalf: 0.3,
      glassInset: 0.015,
      bodyHalf: hw - 0.04,
      sideOut,
      plateHalf: 0.3,
    },
    s.tinted(game.body),
    s.tinted(game.side),
  );
  for (const side of [1, -1] as const)
    for (const { outline, surface, lift } of SIDE_ART[game.title](game, s)) {
      const out0 = sideOut + lift * DECAL_LIFT;
      sideShape(kitAt, side, outline, out0, out0 + DECAL_LIFT, surface);
    }
  blinkPicture(
    k,
    s,
    textRows(game.title),
    [-0.28, 0.28, 1.33, 1.47],
    parts.pixD,
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
    parts.pixD,
    4,
    (ch) => demoTint[ch] ?? null,
  );
  // The marquee.
  const md = at(ARCADE_PROFILE, 6)[0];
  k.panel(-0.34, 0.34, md + DECAL_LIFT, 1.62, 1.86, s.signal(game.accent));
  const title = textRows(game.title);
  const m = fit(title, -0.31, 0.31, 1.66, 1.82);
  const ink = s.tinted(MARQUEE_INK);
  pixelPanel(k, title, m.left, m.top, m.px, md + 2 * DECAL_LIFT, (ch) =>
    ch === "#" ? ink : null,
  );
  // The joystick and the buttons.
  const t = 0.45;
  const shaft = onDeck(k, parts, -0.15, t, 0.011, 0.1, 6, s.metal);
  const ballH = shaft.top;
  k.lathe(
    -0.15,
    shaft.d,
    [
      [0, ballH - 0.01],
      [0.02, ballH - 0.004],
      [0.028, ballH + 0.015],
      [0.02, ballH + 0.034],
      [0, ballH + 0.04],
    ],
    8,
    accent,
  );
  for (const a of [0.04, 0.12, 0.2])
    onDeck(k, parts, a, t, 0.024, 0.012, 8, accent);
  // The coin door with its two slots.
  const front = at(ARCADE_PROFILE, 1)[0];
  k.box(-0.12, 0.12, front, front + 0.02, 0.3, 0.6, s.dark);
  for (const a of [-0.05, 0.05])
    k.box(
      a - 0.012,
      a + 0.012,
      front + 0.02,
      front + 0.02 + DECAL_LIFT,
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

/** The stars on the recruitment marquee: `a` across it and `u` up the hood's face (0 to 1). */
const HOOD_STARS: readonly (readonly [a: number, u: number])[] = [
  [-0.3, 0.3],
  [-0.12, 0.7],
  [0.04, 0.4],
  [0.2, 0.75],
  [0.32, 0.35],
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

/** Half the side of a star square, in metres. */
const STAR_HALF = 0.009;

/**
 * The recruitment cabinet, backed against its wall, wider and deeper than
 * the arcade cabinet, with a long sloped side, built on `cabinet`:
 * - The body in blue-black over `a` -0.46 to 0.46: a deep control deck
 *   rising from h 0.82 to 1.02, the screen slope, a hood over the screen
 *   and the top at 2.0. Over the screen's width (`a` +-0.38) the slope
 *   stands upright at d 0.62 from h 1.25 to 1.7.
 * - The side panels (`a` +-0.46 out to one `DECAL_LIFT` inside the
 *   footprint's edge) glow a steady deep blue-violet (`s.signal`), with
 *   brighter star dots (small signal squares set into the panel from
 *   inside, so they touch the body) and a fighter silhouette (a flat dark
 *   arrowhead with two swept wings) standing `DECAL_LIFT` proud on their
 *   outer faces.
 * - The screen: `VOID WING` in the upper band (groups 0 to 3) and the
 *   demo (`RECRUIT_DEMO`, groups 4 to 7) in the lower band, in the swap
 *   bank.
 * - A plain glowing marquee plate on the hood's face with a few small
 *   star squares on it, and no text.
 * - On the deck plate a flight stick (a column on a boot with a T grip)
 *   and two red fire buttons.
 */
const recruitCabinet: HeroRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw } = heroHalf(kind, variant);
  const bodyHalf = hw - 0.04;
  const sideOut = hw - DECAL_LIFT;
  const glow = s.signal(SIDE_VIOLET);
  const parts = cabinet(
    k,
    kitAt,
    s,
    {
      profile: RECRUIT_PROFILE,
      slopeFoot: 4,
      deckFront: 2,
      screen: { d: 0.62, h0: 1.25, h1: 1.7 },
      screenHalf: 0.38,
      glassHalf: 0.35,
      glassInset: 0.01,
      bodyHalf,
      sideOut,
      plateHalf: 0.4,
    },
    s.tinted(RECRUIT_BLACK),
    glow,
  );
  // The stars and the fighter on the glowing side panels.
  const star = s.signal(STAR_WHITE);
  const r = STAR_HALF;
  for (const side of [1, -1] as const) {
    for (const [d, h] of SIDE_STARS)
      sideShape(
        kitAt,
        side,
        rect(d - r, d + r, h - r, h + r),
        bodyHalf + 0.015,
        hw,
        star,
      );
    sideShape(kitAt, side, FIGHTER, sideOut, hw, s.tinted(FIGHTER_INK));
  }
  // The screen.
  blinkPicture(
    k,
    s,
    textRows(RECRUIT_TITLE),
    [-0.33, 0.33, 1.56, 1.67],
    parts.pixD,
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
    parts.pixD,
    4,
    (ch) => demoTint[ch] ?? null,
  );
  // The marquee plate on the hood's face, and its star squares.
  const p = at(RECRUIT_PROFILE, 6);
  const q = at(RECRUIT_PROFILE, 7);
  const hood = onFace(p, q);
  const du = STAR_HALF / Math.hypot(q[0] - p[0], q[1] - p[1]);
  profileAlong(
    kitAt,
    ORIGIN,
    [
      hood(0.08, 0),
      hood(0.92, 0),
      hood(0.92, DECAL_LIFT),
      hood(0.08, DECAL_LIFT),
    ],
    -0.38,
    0.38,
    s.signal(MARQUEE_VIOLET),
  );
  for (const [a, u] of HOOD_STARS)
    profileAlong(
      kitAt,
      ORIGIN,
      [
        hood(u - du, -0.01),
        hood(u + du, -0.01),
        hood(u + du, 2 * DECAL_LIFT),
        hood(u - du, 2 * DECAL_LIFT),
      ],
      a - STAR_HALF,
      a + STAR_HALF,
      star,
    );
  // The flight stick and the fire buttons.
  const boot = onDeck(k, parts, 0, 0.49, 0.05, 0.03, 10, s.dark);
  k.cylinder(0, boot.d, boot.top, boot.top + 0.16, 0.02, 8, s.metal);
  k.cylinderAlong(-0.1, 0.1, boot.d, boot.top + 0.17, 0.022, 8, s.dark);
  for (const a of [-0.26, 0.26])
    onDeck(k, parts, a, 0.4, 0.032, 0.015, 10, s.tinted(FIRE_RED));
};

/** The arcade kinds' recipes. */
export const ARCADE_RECIPES = {
  "arcade-cabinet": arcadeCabinet,
  "recruit-cabinet": recruitCabinet,
} satisfies Record<
  Extract<HeroKind, "arcade-cabinet" | "recruit-cabinet">,
  HeroRecipe
>;
