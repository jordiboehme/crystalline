/**
 * The retro desk curios' recipes (2.6b): the pocket console standing on its
 * foot, the home-computer tape drive, the portable tape player with its
 * headphones, the video tape (in its sleeve, or lying bare beside it) and
 * the beige laptop. Colours and helpers stay in this file, which imports
 * only `common.ts` of the curio batches.
 *
 * Each follows its original's shape closely. The console, the tape drive
 * and the tape player carry their originals' badges (2.6f C13), the
 * strings of `MARKS` (`marks.ts`); the video tape and the laptop carry
 * none of their originals' names:
 * - The console is a tall light grey brick standing upright on a small dark
 *   foot (C15), the lower half 4 mm thinner at the front. Its upper half
 *   holds a grey-green bezel round a pale yellow-green screen sunk into
 *   the body, showing dark pixels, with a red battery light on the bezel
 *   left of the screen. Below: a black cross
 *   pad, two round maroon buttons on a slant, two small dark oblong
 *   buttons in the middle and five slanted speaker slits bottom right. The
 *   screen swaps two block-pixel pictures of our own (C16): a title word
 *   over a row of blocks (`CONSOLE_TITLE`) and a well with a stack and a
 *   falling piece (`CONSOLE_PLAY`).
 * - The tape drive is a low beige wedge with a lid (four thin raised ribs
 *   at the back, a smoky cassette window with two pale reel hubs) and a
 *   flat key deck at the front, 0.045 high, with five chunky brown piano
 *   keys along its front edge (the first one darker), a
 *   small counter window with a pale strip and a reset knob, and a thick
 *   dark cable leaving the back and lying along it.
 * - The tape player lies flat: silver faces over a dark blue band round
 *   its edges, a lid window showing a cassette (a pale label strip, two
 *   dark hubs), a row of small buttons at its top end with one bright
 *   orange, two jacks and a slide switch on its side. Beside it lie the
 *   headphones: two thin dark earcups with thick orange foam pads, joined
 *   by a thin wire band lying on the surface, and a thin cable from one
 *   cup to a jack.
 * - The video tape is a black brick in a plain cardboard sleeve open at
 *   one short end, its spine and blank white label showing there (v0), or
 *   lying bare beside its empty sleeve (v1) with two reel windows showing
 *   tan tape on light hubs, a lighter front flap and the spine label.
 * - The laptop is a thick beige base with bevelled edges, a raised band at
 *   the back holding two dark drive slots either side of a small latch, a
 *   dark key well with 52 chunky light keys in five rows (the front row
 *   with a long bar), a plain palm rest in front of it, and a beige lid tilted back 15 degrees from vertical
 *   carrying the grey-green screen in a wide bezel in front and a block
 *   pixel "<=>" on its back (C17).
 *
 * The badges (C17): the console's two words side by side on the grey
 * under its bezel, off the blinking screen, as `pixelPanel` quads
 * `MARK_PROUD` in front of the face. The drive's and the player's lie on
 * faces that look up, where no quad can lie (a kit frame only turns about
 * the upright), so each is built from thin pieces `MARK_PROUD` thick
 * instead: the drive's word as slabs on the lid's slope in front of its
 * window, the player's two words as boxes on its lid, the maker's by the
 * buttons and the model's at the far end. Each piece is a run of lit
 * cells merged with the runs straight under it (`pixelRects`), which
 * keeps the player inside the curio budget. Both read from the curio's
 * front (`+d`), their first row towards `-d`.
 *
 * The laptop's lid is the one part that tilts. The kit has no pitched
 * frames, so the lid, its screen and every run of the mark are
 * `profileAlong` slabs of quadrilaterals in the lid's own `(s, o)` terms
 * (`LAPTOP_LID`): `s` up the lid from its hinge edge, `o` through it from
 * its back face (0) to its front face (`thickness`).
 *
 * Only the console blinks (C16): its title picture is groups 0 to 3 and
 * its play picture groups 4 to 7 of the `swap` bank, one group per column
 * quarter (`blinkPicture`), and no cell is in both. Its pixels are dark
 * on a pale screen, so a picture shows while its own groups are low (see
 * `CONSOLE_SCREEN`). The screen and the battery light are steady
 * `s.signal`s and the laptop's screen a steady `s.glow`; nothing else
 * lights.
 */

import type { CurioKind } from "../../../world/types";
import type { Surface } from "../../geometry";
import { DECAL_LIFT, frameAt, type Frame, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import {
  profileAlong,
  tiltedBar,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import {
  blinkPicture,
  fit,
  MARK_PROUD,
  pixelPanel,
  pixelRuns,
  textRows,
} from "../heroes/pixels";
import { MARKS } from "../marks";
import { curioHalf, type CurioRecipe } from "./common";

/** A point of a side profile: depth `d`, height `h`. */
type DH = readonly [d: number, h: number];

/** The frame every curio is built in: the origin at turn 0. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

// --- Shared helpers --------------------------------------------------------

/**
 * A short cylinder whose axis runs along `d` from `d0` to `d1` at `(a,
 * h)`: `cylinderAlong` in a frame yawed a quarter, whose `along` is the
 * old `inward`. A point `(a', d')` of that frame is `(a - d', a')` in the
 * origin's terms, so the axis at `d' = 0` lies at `a`. For round buttons
 * on an upright face, jack sockets and plugs.
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

/**
 * A flat bar lying in plan from `(a0, d0)` to `(a1, d1)`, `width` wide and
 * running `h0` to `h1` in height, stretched `width / 2` past each end so a
 * chain of bars closes its corners: a box in a frame yawed along the bar.
 * For the headphones' wire band and cable.
 */
function planBar(
  kitAt: KitAt,
  a0: number,
  d0: number,
  a1: number,
  d1: number,
  width: number,
  h0: number,
  h1: number,
  s: Surface,
): void {
  const len = Math.hypot(a1 - a0, d1 - d0) / 2 + width / 2;
  const angle = Math.atan2(d1 - d0, a1 - a0);
  kitAt(yawed(ORIGIN, (a0 + a1) / 2, (d0 + d1) / 2, angle)).box(
    -len,
    len,
    -width / 2,
    width / 2,
    h0,
    h1,
    s,
  );
}

/** A rectangle of a pixel picture's lit cells: its first column, its top row, its width and its height in cells. */
interface PixelRect {
  col: number;
  row: number;
  len: number;
  rows: number;
}

/**
 * A pixel picture's lit cells (`#`) as rectangles: each horizontal run of
 * `pixelRuns`, merged with the runs straight under it that start and end
 * in the same columns, so an upright stroke is one piece however tall.
 * Together they cover every lit cell exactly once. For a mark built from
 * boxes or slabs, which cost a dozen triangles a piece.
 */
function pixelRects(rows: readonly string[]): PixelRect[] {
  const rects: PixelRect[] = [];
  for (const r of pixelRuns(rows)) {
    const above = rects.find(
      (x) => x.col === r.col && x.len === r.len && x.row + x.rows === r.row,
    );
    if (above) above.rows++;
    else rects.push({ ...r, rows: 1 });
  }
  return rects;
}

/**
 * Lays `text` flat on a face that looks up, fitted into `a0..a1` along
 * and `d0..d1` across with square pixels as large as both allow (`fit`),
 * centred: `lay` draws one rectangle of lit cells (`pixelRects`) from
 * `a0` to `a1` and `d0` to `d1`. The first row lies towards `-d`, so the
 * text reads from the curio's front (`+d`), left to right along `+a`.
 * Returns the pixel size, to hold to C13's 1 mm floor.
 */
function flatText(
  text: string,
  box: readonly [a0: number, a1: number, d0: number, d1: number],
  lay: (a0: number, a1: number, d0: number, d1: number) => void,
): number {
  const rows = textRows(text);
  const { px, left } = fit(rows, ...box);
  const far = (box[2] + box[3]) / 2 - (rows.length * px) / 2;
  for (const r of pixelRects(rows))
    lay(
      left + r.col * px,
      left + (r.col + r.len) * px,
      far + r.row * px,
      far + (r.row + r.rows) * px,
    );
  return px;
}

// --- Pocket console --------------------------------------------------------

/** The console's body: a light warm grey. */
const CONSOLE_BODY: Rgb = [0.72, 0.71, 0.68];

/** The screen's bezel: a darker grey-green round the screen. */
const CONSOLE_BEZEL: Rgb = [0.55, 0.6, 0.52];

/**
 * The screen: a pale yellow-green, and the colour of its pixels too. The
 * screen is a steady `s.signal` and each pixel a blinking light of the
 * same tint, so a pixel whose group is lit matches the screen and vanishes
 * into it, and one whose group is low (`BLINK_LOW`) reads as a dark olive
 * dot: dark pixels on a pale screen, like the original's unlit display.
 * Shown through `SIGNAL_GAIN`, this tint lands near the original's pale
 * (0.75, 0.8, 0.6).
 */
export const CONSOLE_SCREEN: Rgb = [0.54, 0.58, 0.42];

/** The cross pad: all but black. */
const CONSOLE_PAD: Rgb = [0.08, 0.08, 0.08];

/** The two round buttons: a dark maroon. */
const CONSOLE_BUTTON: Rgb = [0.35, 0.12, 0.15];

/** The two small oblong buttons and the power switch: a dark grey. */
const CONSOLE_SMALL: Rgb = [0.2, 0.2, 0.2];

/** The battery light: red. */
export const CONSOLE_LED: Rgb = [0.8, 0.1, 0.1];

/** The speaker slits: a grey a step darker than the body, a groove in shade. */
const CONSOLE_SLIT: Rgb = [0.38, 0.38, 0.36];

/** The foot stand: a dark charcoal. */
const CONSOLE_STAND: Rgb = [0.12, 0.12, 0.13];

/** The badge's letters under the bezel: a dark navy, as the original prints them. */
export const CONSOLE_BADGE_INK: Rgb = [0.1, 0.12, 0.32];

/**
 * The console's badge (2.6f C13): the box `[a0, a1, h0, h1]` both words
 * are fitted into together, on the grey front between the lower half's
 * top (`CONSOLE.mid`, 0.08) and the bezel's foot (0.088), and the dark
 * columns between the maker's word on the left and the console's name on
 * the right, wider than the space inside the name so the two read as
 * two marks. Both words share one pixel, about 1.2 mm, which the width
 * sets.
 */
const CONSOLE_BADGE = {
  box: [-0.042, 0.042, 0.0805, 0.0875],
  gap: 6,
} as const;

/**
 * The console's layout in metres (`a` across, `d` out of its face, `h`
 * up), the body 0.09 wide, 0.032 deep and 0.148 tall:
 * - `plate`: the foot plate's height (the plate fills the curio's box);
 * - `back`, `front`: the body's back and its upper front face; the lower
 *   front stands `step` further back;
 * - `mid`: where the thinner lower half meets the upper half;
 * - `half`: the body's half width;
 * - `switchH`: the power switch on the body's top, which ends at the
 *   curio's top, so the body ends that much under it;
 * - `bezel` and `lcd`: the bezel's and the screen window's `[a0, a1, h0,
 *   h1]`;
 * - `bezelFront`: the bezel's face, 1 mm under the body's;
 * - `pixD`: the depth of the screen's pixels, half a millimetre under the
 *   bezel's face; the screen itself stands `DECAL_LIFT` behind them, sunk
 *   into the body, so the pixels sit in the glass plane with the pale
 *   screen behind them.
 */
const CONSOLE = {
  plate: 0.006,
  back: -0.018,
  front: 0.014,
  step: 0.004,
  mid: 0.08,
  half: 0.045,
  switchH: 0.001,
  bezel: [-0.037, 0.037, 0.088, 0.146],
  lcd: [-0.025, 0.025, 0.094, 0.14],
  bezelFront: 0.013,
  pixD: 0.0125,
} as const;

/** The pictures' width in pixels. */
const SCREEN_COLS = 17;

/** A dark row of a picture. */
const DARK_ROW = ".".repeat(SCREEN_COLS);

/**
 * The console screen's title picture: the word BLOK in the block-pixel
 * font over a row of four blocks, 17 by 16 pixels, `#` a pixel. Blink
 * groups 0 to 3 of the swap bank, one per column quarter. No cell of it is a pixel in
 * `CONSOLE_PLAY` too, so the two pictures never share a quad.
 */
export const CONSOLE_TITLE: readonly string[] = [
  DARK_ROW,
  DARK_ROW,
  DARK_ROW,
  ...textRows("BLOK").map((r) => `.${r}.`),
  DARK_ROW,
  DARK_ROW,
  "...##.##.##.##...",
  DARK_ROW,
  DARK_ROW,
  DARK_ROW,
  DARK_ROW,
  DARK_ROW,
];

/**
 * The console screen's play picture: a well (two walls and a floor), a
 * ragged stack of blocks at its bottom and one T-shaped piece falling near
 * its top, 17 by 16 pixels, `#` a pixel. Blink groups 4 to 7 of the swap
 * bank, one per column quarter.
 */
export const CONSOLE_PLAY: readonly string[] = [
  "#...............#",
  "#......###......#",
  "#.......#.......#",
  "#...............#",
  "#...............#",
  "#...............#",
  "#...............#",
  "#...............#",
  "#...............#",
  "#...............#",
  "#...............#",
  "#...........##..#",
  "#.##.......####.#",
  "#####.##.########",
  "###.#############",
  "#################",
];

/**
 * The pocket console, upright on its foot (C15), its screen facing `+d`.
 * The plate fills the curio's box; a lip at its front and a slanted prop
 * behind hold the body. The upper half is built round its screen: a back
 * slab, a body-coloured frame round the bezel, the bezel as a frame round
 * the screen window and the pale screen at the bottom of that window.
 */
const pocketConsole: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, hd, top } = curioHalf(kind, variant);
  const C = CONSOLE;
  const body = s.tinted(CONSOLE_BODY);
  const stand = s.tinted(CONSOLE_STAND);
  const bodyTop = top - C.switchH;
  const lowFront = C.front - C.step;

  // The foot: plate, front lip and back prop.
  k.box(-hw, hw, -hd, hd, 0, C.plate, stand);
  k.box(-0.04, 0.04, lowFront, lowFront + 0.006, C.plate, 0.012, stand);
  const propTop: DH = [C.back, 0.075];
  const propFoot: DH = [-hd + 0.003, C.plate];
  profileAlong(
    kitAt,
    ORIGIN,
    tiltedBar(
      (propTop[0] + propFoot[0]) / 2,
      (propTop[1] + propFoot[1]) / 2,
      Math.atan2(propTop[1] - propFoot[1], propTop[0] - propFoot[0]),
      Math.hypot(propTop[0] - propFoot[0], propTop[1] - propFoot[1]),
      0.004,
    ),
    -0.02,
    0.02,
    stand,
  );

  // The body: the thinner lower half, then the upper half round its screen.
  const w = C.half;
  const [b0, b1, bh0, bh1] = C.bezel;
  const [l0, l1, lh0, lh1] = C.lcd;
  const lcdFront = C.pixD - DECAL_LIFT;
  const sunk = lcdFront - 0.001;
  k.box(-w, w, C.back, lowFront, C.plate, C.mid, body);
  k.box(-w, w, C.back, sunk, C.mid, bodyTop, body);
  k.box(-w, b0, sunk, C.front, C.mid, bodyTop, body);
  k.box(b1, w, sunk, C.front, C.mid, bodyTop, body);
  k.box(b0, b1, sunk, C.front, bh1, bodyTop, body);
  k.box(b0, b1, sunk, C.front, C.mid, bh0, body);
  const bezel = s.tinted(CONSOLE_BEZEL);
  k.box(b0, l0, sunk, C.bezelFront, bh0, bh1, bezel);
  k.box(l1, b1, sunk, C.bezelFront, bh0, bh1, bezel);
  k.box(l0, l1, sunk, C.bezelFront, lh1, bh1, bezel);
  k.box(l0, l1, sunk, C.bezelFront, bh0, lh0, bezel);
  k.box(l0, l1, sunk, lcdFront, lh0, lh1, s.signal(CONSOLE_SCREEN));

  // The battery light on the bezel, left of the screen.
  const ledA = (b0 + l0) / 2;
  const ledH = 0.124;
  k.box(
    ledA - 0.0015,
    ledA + 0.0015,
    C.bezelFront,
    C.bezelFront + 0.0015,
    ledH - 0.0015,
    ledH + 0.0015,
    s.signal(CONSOLE_LED),
  );

  // The power switch on the top edge, which is the curio's highest point.
  k.box(-0.035, -0.022, -0.008, 0.002, bodyTop, top, s.tinted(CONSOLE_SMALL));

  // The controls on the lower front.
  const face = lowFront;
  const pad = s.tinted(CONSOLE_PAD);
  const [padA, padH, arm, thick] = [-0.024, 0.052, 0.0105, 0.0035];
  k.box(
    padA - arm,
    padA + arm,
    face,
    face + 0.0035,
    padH - thick,
    padH + thick,
    pad,
  );
  k.box(
    padA - thick,
    padA + thick,
    face,
    face + 0.0035,
    padH - arm,
    padH + arm,
    pad,
  );
  const button = s.tinted(CONSOLE_BUTTON);
  for (const [a, h] of [
    [0.013, 0.044],
    [0.031, 0.052],
  ] as const)
    cylinderAlongD(kitAt, a, h, face, face + 0.0035, 0.0052, 8, button);
  const small = s.tinted(CONSOLE_SMALL);
  for (const a of [-0.009, 0.005])
    k.extrude(
      tiltedBar(a, 0.028, 0.44, 0.011, 0.0032),
      face,
      face + 0.002,
      small,
    );
  const slit = s.tinted(CONSOLE_SLIT);
  for (let i = 0; i < 5; i++)
    k.extrude(
      tiltedBar(0.021 + i * 0.0045, 0.022, 1.05, 0.018, 0.0014),
      face,
      face + 0.0012,
      slit,
    );

  // The screen's two pictures, which the swap bank shows in turn: a
  // picture's pixels go dark while its own groups are low, so the title
  // (groups 0 to 3) shows while the play picture's groups are lit and
  // melt into the screen, and the other way round.
  const screen = [
    l0 + 0.0015,
    l1 - 0.0015,
    lh0 + 0.0015,
    lh1 - 0.0015,
  ] as const;
  const ink = (ch: string) => (ch === "#" ? CONSOLE_SCREEN : null);
  blinkPicture(k, s, CONSOLE_TITLE, screen, C.pixD, 0, ink);
  blinkPicture(k, s, CONSOLE_PLAY, screen, C.pixD, 4, ink);

  // The badge under the bezel, the maker's word left of the console's
  // name: two marks at one pixel, steady, off the screen's blink bank.
  const [maker, name] = MARKS.consoleBadge.map((w) => textRows(w));
  const both = (maker ?? []).map(
    (r, i) => r + ".".repeat(CONSOLE_BADGE.gap) + (name?.[i] ?? ""),
  );
  const at = fit(both, ...CONSOLE_BADGE.box);
  const badge = s.tinted(CONSOLE_BADGE_INK);
  const letters = (ch: string) => (ch === "#" ? badge : null);
  const badgeFace = C.front + MARK_PROUD;
  pixelPanel(k, maker ?? [], at.left, at.top, at.px, badgeFace, letters);
  const nameLeft =
    at.left + ((maker?.[0]?.length ?? 0) + CONSOLE_BADGE.gap) * at.px;
  pixelPanel(k, name ?? [], nameLeft, at.top, at.px, badgeFace, letters);
};

// --- Tape drive ------------------------------------------------------------

/** The tape drive's body and lid: a warm beige. */
const DRIVE_BODY: Rgb = [0.78, 0.73, 0.62];

/** The piano keys: a brown. */
export const DRIVE_KEY: Rgb = [0.45, 0.32, 0.22];

/** The record key, first in the row: a darker brown. */
export const DRIVE_RECORD_KEY: Rgb = [0.3, 0.2, 0.14];

/** The cassette window and the counter: a dark smoky grey. */
export const DRIVE_WINDOW: Rgb = [0.15, 0.14, 0.13];

/** The badge's letters on the lid: a dark warm brown-grey. */
export const DRIVE_BADGE_INK: Rgb = [0.24, 0.21, 0.18];

/**
 * The drive's badge (2.6f C13): the box `[a0, a1, d0, d1]` its word is
 * fitted into, on the lid's strip between the window's front edge (0.03)
 * and the lid's own front (`DRIVE.lidFront`, 0.04), at the left, behind
 * the record key. The strip's depth sets the pixel, 1.4 mm.
 */
const DRIVE_BADGE = [-0.088, -0.03, 0.0315, 0.0385] as const;

/** The reel hubs behind the window and the counter's strip: a pale grey. */
const DRIVE_HUB: Rgb = [0.8, 0.8, 0.78];

/** The cable: all but black. */
const DRIVE_CABLE: Rgb = [0.1, 0.1, 0.1];

/**
 * The tape drive's layout in metres:
 * - `cableRoom`: the strip behind the body the cable lies in, so the body
 *   is that much shallower than the curio's box (0.139 deep, not 0.15);
 * - `fall`: how much lower the body's top is at the front than at the
 *   back, the wedge;
 * - `lid`, `rib`: the lid's thickness and the ribs' rise above it, so the
 *   body's back top is `top - rib - lid` and the rearmost rib reaches the
 *   curio's top;
 * - `lidFront`: where the lid ends and the key deck begins;
 * - `deck`: the key deck's height, a flat step from the lid's front to the
 *   body's front edge, so the front stands at 0.045 as the original's does
 *   (the wedge's own slope would leave it at 0.041);
 * - `cable`: the cable's radius.
 */
const DRIVE = {
  cableRoom: 0.011,
  fall: 0.01,
  lid: 0.003,
  rib: 0.001,
  lidFront: 0.04,
  deck: 0.045,
  cable: 0.0045,
} as const;

/** The piano keys' count, first the record key. */
const DRIVE_KEYS = 5;

/**
 * The tape drive, lying flat with its keys along the front. The body is a
 * wedge (`profileAlong`) whose front strip is a flat key deck, the lid a
 * slab on its sloped top from the back edge to the deck, the ribs and the window thin slabs on the lid, so
 * each follows the slope.
 */
const tapeDrive: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, hd, top } = curioHalf(kind, variant);
  const D = DRIVE;
  const back = -hd + D.cableRoom;
  const backH = top - D.rib - D.lid;
  const bodyH = (d: number) => backH - ((d - back) / (hd - back)) * D.fall;
  const lidH = (d: number) => bodyH(d) + D.lid;
  const body = s.tinted(DRIVE_BODY);
  /** A slab on the slope from `d0` to `d1`, `lo` to `hi` above the line `at`. */
  const onSlope = (
    at: (d: number) => number,
    d0: number,
    d1: number,
    lo: number,
    hi: number,
    a0: number,
    a1: number,
    sf: Surface,
  ) =>
    profileAlong(
      kitAt,
      ORIGIN,
      [
        [d0, at(d0) + lo],
        [d1, at(d1) + lo],
        [d1, at(d1) + hi],
        [d0, at(d0) + hi],
      ],
      a0,
      a1,
      sf,
    );

  profileAlong(
    kitAt,
    ORIGIN,
    [
      [back, 0],
      [hd, 0],
      [hd, D.deck],
      [D.lidFront, D.deck],
      [D.lidFront, bodyH(D.lidFront)],
      [back, backH],
    ],
    -hw,
    hw,
    body,
  );
  onSlope(bodyH, back, D.lidFront, 0, D.lid, -0.094, 0.094, body);
  for (let i = 0; i < 4; i++) {
    const d0 = back + 0.0045 + i * 0.007;
    onSlope(lidH, d0, d0 + 0.003, -0.0005, D.rib, -0.085, 0.085, body);
  }
  onSlope(
    lidH,
    -0.03,
    0.03,
    -0.0005,
    0.0006,
    -0.06,
    0.06,
    s.tinted(DRIVE_WINDOW),
  );
  const hub = s.tinted(DRIVE_HUB);
  for (const a of [-0.028, 0.028])
    k.cylinder(a, 0, lidH(0.007) - 0.001, lidH(-0.007) + 0.0014, 0.007, 8, hub);

  // Five piano keys along the front edge, the record key first and darker.
  for (let i = 0; i < DRIVE_KEYS; i++) {
    const a0 = -0.09 + i * 0.025;
    const key = s.tinted(i === 0 ? DRIVE_RECORD_KEY : DRIVE_KEY);
    k.box(
      a0,
      a0 + 0.022,
      0.046,
      hd - 0.0005,
      D.deck - 0.001,
      D.deck + 0.007,
      key,
    );
  }

  // The counter window with its pale strip, and its reset knob.
  k.box(
    0.04,
    0.072,
    0.048,
    0.072,
    D.deck - 0.001,
    D.deck + 0.004,
    s.tinted(DRIVE_WINDOW),
  );
  k.box(0.045, 0.067, 0.054, 0.066, D.deck + 0.004, D.deck + 0.0045, hub);
  k.cylinder(
    0.085,
    0.06,
    D.deck - 0.001,
    D.deck + 0.004,
    0.004,
    8,
    s.tinted(DRIVE_KEY),
  );

  // The cable: a grommet on the back, a stub out of it and a run along it.
  const cable = s.tinted(DRIVE_CABLE);
  const r = D.cable;
  const exitA = 0.07;
  const runD = -hd + r + 0.0005;
  k.box(exitA - 0.008, exitA + 0.008, back - 0.003, back, 0, 0.012, cable);
  cylinderAlongD(kitAt, exitA, r, runD, back, r, 6, cable);
  k.cylinderAlong(-0.03, exitA + r, runD, r, r, 6, cable);

  // The badge on the lid's front strip: each piece a slab on the slope,
  // standing `MARK_PROUD` off the lid.
  const badge = s.tinted(DRIVE_BADGE_INK);
  flatText(MARKS.driveBadge, DRIVE_BADGE, (a0, a1, d0, d1) =>
    onSlope(lidH, d0, d1, 0, MARK_PROUD, a0, a1, badge),
  );
};

// --- Tape player -----------------------------------------------------------

/** The player's faces: brushed silver. */
const PLAYER_SILVER: Rgb = [0.75, 0.75, 0.77];

/** The band round the player's edges: a dark blue. */
const PLAYER_TRIM: Rgb = [0.12, 0.18, 0.4];

/** The one bright button: orange. */
export const PLAYER_ORANGE: Rgb = [0.9, 0.45, 0.08];

/** The other buttons, the jacks, the switch, the cassette hubs and the earcups: a dark grey. */
const PLAYER_DARK: Rgb = [0.15, 0.15, 0.15];

/** The headphones' foam pads: orange. */
export const PLAYER_FOAM: Rgb = [0.95, 0.5, 0.1];

/** The cassette's label strip seen through the lid: an off white. */
export const PLAYER_LABEL: Rgb = [0.85, 0.84, 0.8];

/** The lid's window: a smoky blue-grey, light enough for the dark hubs to show. */
export const PLAYER_SMOKE: Rgb = [0.3, 0.31, 0.35];

/** The badges' letters on the lid: a dark navy on the silver. */
export const PLAYER_BADGE_INK: Rgb = [0.07, 0.09, 0.2];

/**
 * The player's two badges (2.6f C13), each the `d0..d1` strip of the lid
 * its word is fitted into, centred along the player: the maker's word
 * between the window's top edge (0.034) and the buttons (0.054), the
 * model's between the far end (-0.067) and the window's bottom edge
 * (-0.058), where the original carries its name label. The strips' depth
 * sets the pixels, 2.4 mm and 1.4 mm.
 */
const PLAYER_BADGE = {
  maker: [0.038, 0.05],
  model: [-0.066, -0.059],
} as const;

/**
 * The tape player's layout in metres:
 * - `width`, `depth`, `height`: the player's body (`a`, `d`, `h`), lying
 *   at the `-a` end of the curio's box;
 * - `low`, `high`: the heights where the blue band round the edges starts
 *   and ends, silver below and above;
 * - `cups`: the headphones' centre `(a, d)`; the earcups lie `band` either
 *   side of it along `a`, joined by the wire band's half circle towards
 *   `+d`;
 * - `cup`, `pad`: the earcups' and the foam pads' radius.
 */
const PLAYER = {
  width: 0.088,
  depth: 0.134,
  height: 0.029,
  low: 0.006,
  high: 0.023,
  cups: [0.05, -0.03],
  band: 0.07,
  cup: 0.02,
  pad: 0.022,
} as const;

/** The buttons on the player's top end, and which one is orange. */
const PLAYER_BUTTONS = 4;

/** The index of the orange button in the row. */
const ORANGE_BUTTON = 3;

/** The wire band's segments round its half circle. */
const BAND_SEGMENTS = 8;

/**
 * The portable tape player lying flat at the curio's `-a` end, its
 * headphones lying beside it. Its buttons stand on the lid at the top
 * (`+d`) end and reach the curio's top; the jacks and the slide switch sit
 * on the side facing the headphones.
 */
const tapePlayer: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hw, top } = curioHalf(kind, variant);
  const P = PLAYER;
  const a0 = -hw;
  const a1 = a0 + P.width;
  const dh = P.depth / 2;
  const silver = s.tinted(PLAYER_SILVER);
  const dark = s.tinted(PLAYER_DARK);

  k.box(a0, a1, -dh, dh, 0, P.low, silver);
  k.box(a0, a1, -dh, dh, P.low, P.high, s.tinted(PLAYER_TRIM));
  k.box(a0, a1, -dh, dh, P.high, P.height, silver);

  // The lid window with the cassette behind it: two hubs on a line along
  // the player, and the pale label strip running beside that line.
  const mid = (a0 + a1) / 2;
  const lidTop = P.height;
  k.box(
    a0 + 0.008,
    a1 - 0.008,
    -0.058,
    0.034,
    lidTop,
    lidTop + 0.0006,
    s.tinted(PLAYER_SMOKE),
  );
  k.box(
    mid + 0.011,
    mid + 0.024,
    -0.052,
    0.014,
    lidTop + 0.0006,
    lidTop + 0.0012,
    s.tinted(PLAYER_LABEL),
  );
  for (const d of [-0.04, 0.002])
    k.cylinder(mid, d, lidTop + 0.0006, lidTop + 0.0014, 0.0065, 6, dark);

  // The badges on the lid, flat boxes `MARK_PROUD` thick.
  const badge = s.tinted(PLAYER_BADGE_INK);
  const [makerWord, modelWord] = MARKS.playerBadge;
  for (const [word, [d0, d1]] of [
    [makerWord, PLAYER_BADGE.maker],
    [modelWord, PLAYER_BADGE.model],
  ] as const)
    flatText(word, [a0 + 0.008, a1 - 0.008, d0, d1], (x0, x1, y0, y1) =>
      k.box(x0, x1, y0, y1, lidTop, lidTop + MARK_PROUD, badge),
    );

  // The row of buttons flush with the top end edge, one orange.
  for (let i = 0; i < PLAYER_BUTTONS; i++) {
    const b0 = a0 + 0.008 + i * 0.018;
    const face = i === ORANGE_BUTTON ? s.tinted(PLAYER_ORANGE) : dark;
    k.box(b0, b0 + 0.014, dh - 0.013, dh, lidTop, top, face);
  }

  // Two jacks and the slide switch on the side facing the headphones.
  const jackH = (P.low + P.high) / 2;
  for (const d of [0.048, 0.058])
    k.cylinderAlong(a1, a1 + 0.0035, d, jackH, 0.0028, 6, dark);
  k.box(a1, a1 + 0.0025, -0.035, -0.018, jackH - 0.0035, jackH + 0.0035, dark);

  // The headphones: earcups with foam pads, the wire band and the cable.
  const [ca, cd] = P.cups;
  const foam = s.tinted(PLAYER_FOAM);
  for (const a of [ca - P.band, ca + P.band]) {
    k.cylinder(a, cd, 0, 0.008, P.cup, 8, dark);
    k.cylinder(a, cd, 0.008, 0.019, P.pad, 10, foam);
  }
  const wire = s.tinted(PLAYER_SILVER);
  for (let i = 0; i < BAND_SEGMENTS; i++) {
    const t0 = (Math.PI * i) / BAND_SEGMENTS;
    const t1 = (Math.PI * (i + 1)) / BAND_SEGMENTS;
    planBar(
      kitAt,
      ca + P.band * Math.cos(t0),
      cd + P.band * Math.sin(t0),
      ca + P.band * Math.cos(t1),
      cd + P.band * Math.sin(t1),
      0.0025,
      0,
      0.003,
      wire,
    );
  }
  const plugD = 0.048;
  const cableW = 0.0022;
  const foot = [a1 + 0.014, cableW / 2] as const;
  planBar(kitAt, ca - P.band, cd, foot[0], plugD, cableW, 0, cableW, dark);
  k.extrude(
    tiltedBar(
      (foot[0] + a1 + 0.008) / 2,
      (foot[1] + jackH) / 2,
      Math.atan2(jackH - foot[1], a1 + 0.008 - foot[0]),
      Math.hypot(jackH - foot[1], a1 + 0.008 - foot[0]),
      cableW,
    ),
    plugD - cableW / 2,
    plugD + cableW / 2,
    dark,
  );
  k.cylinderAlong(a1 + 0.002, a1 + 0.01, plugD, jackH, 0.0022, 6, dark);
};

// --- Video tape ------------------------------------------------------------

/** The sleeve: plain brown cardboard. */
const TAPE_SLEEVE: Rgb = [0.72, 0.6, 0.42];

/** The tape's shell: black plastic. */
const TAPE_BLACK: Rgb = [0.04, 0.04, 0.04];

/** The blank spine label: an off white. */
const TAPE_LABEL: Rgb = [0.92, 0.9, 0.85];

/** The tape wound on the reels: tan. */
export const TAPE_REEL: Rgb = [0.6, 0.55, 0.5];

/** The front flap: a black a shade lighter than the shell. */
const TAPE_FLAP: Rgb = [0.08, 0.08, 0.08];

/** The reel windows: a smoky dark grey. */
const TAPE_WINDOW: Rgb = [0.16, 0.16, 0.18];

/** The reels' hubs: a light grey-white. */
const TAPE_HUB: Rgb = [0.85, 0.85, 0.82];

/**
 * The video tape's sizes in metres: the tape (`length` along `a`, `depth`
 * along `d`, `height`), the sleeve round it (`sleeveLength`,
 * `sleeveDepth`; its height is the curio's top) and the cardboard's
 * thickness (`wall`).
 */
const VIDEO = {
  length: 0.187,
  depth: 0.103,
  height: 0.025,
  sleeveLength: 0.195,
  sleeveDepth: 0.108,
  wall: 0.0015,
} as const;

/**
 * An empty sleeve from `a0` to `a0 + sleeveLength`, lying flat, `top`
 * tall, open at its `open` end (`1` the `+a` end, `-1` the `-a` end): a
 * bottom, a top, two long sides and the closed end.
 */
function sleeve(
  k: Kit,
  s: Surfaces,
  a0: number,
  top: number,
  open: 1 | -1,
): void {
  const V = VIDEO;
  const a1 = a0 + V.sleeveLength;
  const dh = V.sleeveDepth / 2;
  const w = V.wall;
  const card = s.tinted(TAPE_SLEEVE);
  k.box(a0, a1, -dh, dh, 0, w, card);
  k.box(a0, a1, -dh, dh, top - w, top, card);
  k.box(a0, a1, -dh, -dh + w, w, top - w, card);
  k.box(a0, a1, dh - w, dh, w, top - w, card);
  const shut = open === 1 ? a0 : a1 - w;
  k.box(shut, shut + w, -dh + w, dh - w, w, top - w, card);
}

/**
 * The blank label on a tape's short end at `a` (facing `+a` when `face`
 * is 1, `-a` when -1): a thin white box 0.8 mm proud of the end, with no
 * print. A box rather than a lifted decal, so it stays inside the sleeve's
 * opening.
 */
function spineLabel(
  k: Kit,
  s: Surfaces,
  a: number,
  face: 1 | -1,
  h0: number,
): void {
  k.box(
    a,
    a + face * 0.0008,
    -0.035,
    0.035,
    h0 + 0.005,
    h0 + 0.02,
    s.tinted(TAPE_LABEL),
  );
}

/**
 * The video tape: in its sleeve with its spine and label showing at the
 * open `+a` end (v0), or lying bare at the `-a` end beside its empty
 * sleeve, whose open end faces it (v1). The bare tape shows two reel
 * windows on its top with tan tape wound on light hubs, and a lighter
 * flap along its front edge.
 */
const videoTape: CurioRecipe = ({ k, s, variant, kind }) => {
  const { hw, top } = curioHalf(kind, variant);
  const V = VIDEO;
  const tape = s.tinted(TAPE_BLACK);
  const dh = V.depth / 2;
  if (variant === 0) {
    const a0 = -V.sleeveLength / 2;
    sleeve(k, s, a0, top, 1);
    const spine = a0 + V.sleeveLength - 0.001;
    k.box(spine - V.length, spine, -dh, dh, V.wall, V.wall + V.height, tape);
    spineLabel(k, s, spine, 1, V.wall);
    return;
  }
  const t0 = -hw + 0.002;
  const t1 = t0 + V.length;
  k.box(t0, t1, -dh, dh, 0, V.height, tape);
  spineLabel(k, s, t0, -1, 0);
  k.box(
    t0 + 0.012,
    t1 - 0.012,
    dh,
    dh + 0.001,
    0.003,
    0.022,
    s.tinted(TAPE_FLAP),
  );
  const mid = (t0 + t1) / 2;
  const h = V.height;
  for (const a of [mid - 0.047, mid + 0.047]) {
    k.box(
      a - 0.027,
      a + 0.027,
      -0.03,
      0.022,
      h,
      h + 0.0005,
      s.tinted(TAPE_WINDOW),
    );
    k.cylinder(a, -0.004, h + 0.0005, h + 0.001, 0.02, 10, s.tinted(TAPE_REEL));
    k.cylinder(a, -0.004, h + 0.001, h + 0.0015, 0.008, 8, s.tinted(TAPE_HUB));
  }
  sleeve(k, s, hw - 0.002 - V.sleeveLength, top, -1);
};

// --- Beige laptop ----------------------------------------------------------

/** The laptop's case: beige. */
const LAPTOP_BEIGE: Rgb = [0.8, 0.75, 0.63];

/** The keys: a light cream. */
export const LAPTOP_KEY: Rgb = [0.88, 0.86, 0.8];

/** The key well round the keys: a dark warm grey. */
const LAPTOP_WELL: Rgb = [0.3, 0.29, 0.27];

/** The drive slots: all but black. */
export const LAPTOP_SLOT: Rgb = [0.1, 0.1, 0.1];

/** The latch between the slots: a beige a shade darker than the case. */
export const LAPTOP_LATCH: Rgb = [0.68, 0.63, 0.52];

/** The monochrome screen: a grey-green, glowing faintly. */
const LAPTOP_LCD: Rgb = [0.6, 0.63, 0.55];

/** The "<=>" mark on the lid's back: a mid grey. */
export const LAPTOP_MARK: Rgb = [0.3, 0.3, 0.3];

/**
 * The laptop's base in metres: `width` along `a`, `depth` along `d` (its
 * front at the curio's `+d` edge), `height`, the `bevel` of its edges, the
 * raised band's `band` depth and `bandH` top, and the key well's
 * `wellMargin` from the case's sides and its `rowPitch`, from one row of
 * keys to the next; the keys stop after five rows and the plain beige
 * palm rest runs from there to the front edge.
 */
const LAPTOP = {
  width: 0.405,
  depth: 0.305,
  height: 0.035,
  bevel: 0.003,
  band: 0.05,
  bandH: 0.045,
  wellMargin: 0.0125,
  rowPitch: 0.031,
} as const;

/**
 * The laptop's lid, in the side view's `(d, h)`: it is hinged along the
 * back top edge of the raised band, its back face's lower edge at
 * `(hingeD, hingeH)`, and tilted back `tilt` radians from vertical. `up`
 * is the unit vector up the lid and `out` the unit normal out of its front
 * face, so a point `s` up the lid and `o` through it from the back face is
 * `hinge + s * up + o * out`. `length` is chosen so the lid's front top
 * corner lands exactly on the curio's top (about 0.2897 m, for the 0.29
 * of the original); `width` is along `a`. Exported so the laptop's test
 * can put the mark back into the lid's terms.
 */
export const LAPTOP_LID = (() => {
  const tilt = (15 * Math.PI) / 180;
  const { hd, top } = curioHalf("beige-laptop", 0);
  const hingeD = hd - LAPTOP.depth + LAPTOP.bevel;
  const hingeH = LAPTOP.bandH;
  const thickness = 0.02;
  const length = (top - hingeH - thickness * Math.sin(tilt)) / Math.cos(tilt);
  return {
    tilt,
    hingeD,
    hingeH,
    thickness,
    length,
    width: 0.4,
    up: [-Math.sin(tilt), Math.cos(tilt)] as const,
    out: [Math.cos(tilt), Math.sin(tilt)] as const,
  };
})();

/** The mark's pixel size on the lid's back, in metres. */
export const MARK_PX = 0.012;

/** The mark's text, in the block-pixel font (C17). */
const MARK_TEXT = "<=>";

/** The keyboard's rows from the back: how many keys each holds; the front row holds the bar. */
const KEY_ROWS = [12, 12, 11, 10] as const;

/** The front row: small keys either side of the bar, and the bar's width in key pitches. */
const BAR_ROW = { side: 3, bar: 6 } as const;

/** A point `s` up the laptop's lid and `o` through it from its back face, in `(d, h)`. */
function lidPoint(s: number, o: number): DH {
  const L = LAPTOP_LID;
  return [
    L.hingeD + s * L.up[0] + o * L.out[0],
    L.hingeH + s * L.up[1] + o * L.out[1],
  ];
}

/** A slab of the lid's tilted outline: `s0..s1` up the lid, `o0..o1` through it, `a0..a1` along. */
function lidSlab(
  kitAt: KitAt,
  s0: number,
  s1: number,
  o0: number,
  o1: number,
  a0: number,
  a1: number,
  sf: Surface,
): void {
  profileAlong(
    kitAt,
    ORIGIN,
    [lidPoint(s0, o0), lidPoint(s1, o0), lidPoint(s1, o1), lidPoint(s0, o1)],
    a0,
    a1,
    sf,
  );
}

/**
 * The beige laptop, keyboard to the front, lid tilted up at the back (it
 * stands only where its front is free, `fixed` in the catalogue).
 *
 * The mark on the lid's back is read from behind, where `+a` is on the
 * viewer's left, so column `c` of the text runs from `+a` towards `-a`:
 * column 0 starts at `MARK_PX * cols / 2` and each run takes `a` from
 * `start - (col + len) * MARK_PX` to `start - col * MARK_PX`. Row 0 is the
 * highest up the lid. Each run is its own slab, sunk 1 mm into the lid and
 * standing `DECAL_LIFT` proud of its back face, centred across the lid and
 * on the middle of its upper half.
 */
const beigeLaptop: CurioRecipe = ({ k, kitAt, s, variant, kind }) => {
  const { hd } = curioHalf(kind, variant);
  const B = LAPTOP;
  const L = LAPTOP_LID;
  const beige = s.tinted(LAPTOP_BEIGE);
  const back = hd - B.depth;
  const half = B.width / 2;
  const inner = half - B.bevel;

  // The base, its raised band, the drive slots and the latch.
  k.bevelBox(-half, half, back, hd, 0, B.height, B.bevel, beige);
  const bandFront = back + B.band;
  k.box(-inner, inner, back + B.bevel, bandFront, B.height, B.bandH, beige);
  const slot = s.tinted(LAPTOP_SLOT);
  const slotH = (B.height + B.bandH) / 2;
  for (const side of [-1, 1] as const) {
    const [x0, x1] = [side * 0.0225, side * 0.1175];
    k.box(
      x0,
      x1,
      bandFront,
      bandFront + 0.0012,
      slotH - 0.002,
      slotH + 0.002,
      slot,
    );
  }
  k.box(
    -0.012,
    0.012,
    bandFront,
    bandFront + 0.003,
    slotH - 0.003,
    slotH + 0.003,
    s.tinted(LAPTOP_LATCH),
  );

  // The key well and the keys: four rows and the front row with the bar,
  // then the plain beige palm rest to the front edge.
  const keyRows = KEY_ROWS.length + 1;
  const wellA = half - B.wellMargin;
  const well0 = bandFront + 0.008;
  const well1 = well0 + 0.008 + keyRows * B.rowPitch;
  const keyH = B.height + 0.001;
  k.box(-wellA, wellA, well0, well1, B.height, keyH, s.tinted(LAPTOP_WELL));
  const pitch = (2 * wellA - 0.004) / 12;
  const keyW = pitch - 0.004;
  const rowPitch = B.rowPitch;
  const keyD = rowPitch - 0.006;
  const key = s.tinted(LAPTOP_KEY);
  const rowD = (r: number) => well0 + 0.004 + r * rowPitch;
  KEY_ROWS.forEach((n, r) => {
    const start = -(n * pitch) / 2;
    for (let i = 0; i < n; i++) {
      const a0 = start + i * pitch + 0.002;
      k.box(a0, a0 + keyW, rowD(r), rowD(r) + keyD, keyH, keyH + 0.005, key);
    }
  });
  const barRow = rowD(KEY_ROWS.length);
  const rowStart = -6 * pitch;
  const cells = [
    ...Array.from({ length: BAR_ROW.side }, (_, i) => [i, 1] as const),
    [BAR_ROW.side, BAR_ROW.bar] as const,
    ...Array.from(
      { length: BAR_ROW.side },
      (_, i) => [BAR_ROW.side + BAR_ROW.bar + i, 1] as const,
    ),
  ];
  for (const [at, span] of cells) {
    const a0 = rowStart + at * pitch + 0.002;
    k.box(
      a0,
      a0 + span * pitch - 0.004,
      barRow,
      barRow + keyD,
      keyH,
      keyH + 0.005,
      key,
    );
  }

  // The lid, the screen in its wide bezel, and the mark on its back.
  lidSlab(kitAt, 0, L.length, 0, L.thickness, -L.width / 2, L.width / 2, beige);
  lidSlab(
    kitAt,
    0.05,
    L.length - 0.04,
    L.thickness - 0.0005,
    L.thickness + 0.001,
    -0.155,
    0.155,
    s.glow(LAPTOP_LCD),
  );
  const rows = textRows(MARK_TEXT);
  const cols = rows[0]?.length ?? 0;
  const start = (cols * MARK_PX) / 2;
  const sTop = 0.75 * L.length + (rows.length * MARK_PX) / 2;
  const mark = s.tinted(LAPTOP_MARK);
  for (const run of pixelRuns(rows))
    lidSlab(
      kitAt,
      sTop - (run.row + 1) * MARK_PX,
      sTop - run.row * MARK_PX,
      0.001,
      -DECAL_LIFT,
      start - (run.col + run.len) * MARK_PX,
      start - run.col * MARK_PX,
      mark,
    );
};

/**
 * The retro kinds' recipes, one per kind: the pocket console, the tape
 * drive, the tape player, the video tape (two variants) and the beige
 * laptop, each described in this module's doc.
 */
export const RETRO_RECIPES = {
  "pocket-console": pocketConsole,
  "tape-drive": tapeDrive,
  "tape-player": tapePlayer,
  "video-tape": videoTape,
  "beige-laptop": beigeLaptop,
} satisfies Record<
  Extract<
    CurioKind,
    | "pocket-console"
    | "tape-drive"
    | "tape-player"
    | "video-tape"
    | "beige-laptop"
  >,
  CurioRecipe
>;
