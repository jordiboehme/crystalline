/**
 * The rare props that carry a painted mark (2.6d): the marked crate and
 * the gravity console. Both carry the capsule maker's round C over its
 * word (`CAPSULE_LOGO`, `MARKS.capsuleWord`) in dark blue (`MARK_BLUE`),
 * drawn with `pixelPanel` at `DECAL_LIFT` off their faces, as every large
 * mark is (C16).
 *
 * - **Marked crate.** A sturdy pale grey cargo crate, 1.0 by 0.8 by 0.8 m,
 *   with dark grey corner guards up all four corners and edge ribs along
 *   the top and bottom edges of all four sides. The mark is stencilled
 *   large on all four sides, as a cargo crate is marked, so whichever
 *   way the crate turns and wherever it is seen from, a mark faces the
 *   viewer. The rare step's mark relabels one
 *   accepted crate (C12): variant 0 stands alone, variant 1 tops a large
 *   plain crate in the look's machine colour, 1.2 by 1.1 by 0.85 m, the
 *   station's own crate body, so the stack is 1.65 m. No light.
 * - **Gravity console.** A short white pillar at waist height on a dark
 *   plinth, under a light grey deck that slants up towards the back. At
 *   the back an upright white facia stands above the deck and carries one
 *   large round dial (a dark rim, a white face, twelve dark ticks and a
 *   red needle) and a small dark screen with the readout
 *   (`MARKS.gravity`) in amber pixels. A row of six chunky square buttons
 *   stands stepped along the slant; two of them carry lit lamp caps, green
 *   and red. The mark is on the pillar's front, below the deck. The
 *   console stands wall-side, backed to the wall and facing the room (C13).
 *
 * The kit has no pitched frame: a part is either upright or flat, and only
 * a side profile (`profileAlong`) can slant. So the deck is a profile and
 * the buttons stand on it as upright boxes stepped with its height, while
 * the dial and the screen, which are discs and panels, sit on the upright
 * facia above the deck's back edge, clear of it, rather than on the slant.
 *
 * Lights (C18): the console's bank is `status` (`PROP_BANK`), a strobe that
 * spends most of its time low. The readout's pixels blink in group 0 and
 * the two lamp caps in groups 1 and 2. Each blinking tint is set above 1
 * so that at `BLINK_LOW` it still shows the lit colour of an unlit lamp (a
 * dim amber, a plain green, a plain red), never a dark hole, and at the
 * flash it clips to a bright one. The other four buttons are plain grey.
 */

import { FOOTPRINTS } from "../../../world/footprints";
import type { FloorPropKind } from "../../../world/types";
import { DECAL_LIFT, frameAt, type Kit } from "../../kit";
import type { Rgb } from "../../looks";
import {
  discOutline,
  profileAlong,
  tiltedBar,
  yawed,
  type KitAt,
  type Surfaces,
} from "../common";
import { fit, pixelPanel, textRows } from "../heroes/pixels";
import { CAPSULE_LOGO, MARK_BLUE, MARKS } from "../marks";
import type { PropRecipe } from "./common";

/** A floor kind's half-extents along `a` and `d`, from its footprint. */
function halfSize(
  kind: FloorPropKind,
  variant: number,
): { hw: number; hd: number } {
  const size = FOOTPRINTS.prop[kind][variant];
  if (!size) throw new Error(`${kind}: no variant ${String(variant)}`);
  return { hw: size.width / 2, hd: size.depth / 2 };
}

/** The frame every recipe here builds in. */
const ORIGIN = frameAt([0, 0, 0], 0);

/** The crate's pale grey-white body. */
const CRATE_WHITE: Rgb = [0.86, 0.87, 0.88];

/** The crate's corner guards and edge ribs: a darker grey. */
const GUARD_GREY: Rgb = [0.45, 0.47, 0.5];

/** The marked crate's own height in metres; its half-width and half-depth come from its variant 0 footprint (`halfSize`). */
const CRATE_H = 0.8;

/** How far the crate's body sits inside its footprint, in metres. */
const CRATE_INSET = 0.015;

/** The corner guards' side, in metres. */
const GUARD = 0.06;

/** The edge ribs' height, in metres; they stand as proud of the body as the guards. */
const RIB_H = 0.04;

/**
 * How far the body's top sits under the guards' and ribs' tops, in
 * metres, so no top face is shared.
 */
const TOP_DROP = 0.003;

/** The round C's pixel size on the crate, in metres: 11 pixels, 0.374 m. */
const CRATE_LOGO_PX = 0.034;

/** The round C's top above the crate's foot, in metres. */
const CRATE_LOGO_TOP = 0.68;

/** The word's pixel size on the crate, in metres: 51 columns, 0.612 m. */
const CRATE_WORD_PX = 0.012;

/** The word's top above the crate's foot, in metres. */
const CRATE_WORD_TOP = 0.25;

/** The stack's base crate: its height and its inset from the footprint, in metres. */
const STACK_BASE_H = 0.85;
const STACK_INSET = 0.02;

/** The gravity console's plinth: dark. */
const PLINTH_DARK: Rgb = [0.25, 0.25, 0.27];

/** The console's pillar and facia: white. */
const PILLAR_WHITE: Rgb = [0.9, 0.9, 0.88];

/** The console's slanted deck: a light grey, a shade under the pillar's white. */
export const DECK_GREY: Rgb = [0.8, 0.8, 0.8];

/** The dial's rim and ticks, the screen's window and the unlit buttons' collars: near black. */
const DIAL_DARK: Rgb = [0.13, 0.13, 0.15];

/** The dial's needle: red. */
export const NEEDLE: Rgb = [0.85, 0.15, 0.1];

/**
 * The readout's pixels, in blink group 0: an amber whose low phase
 * (`BLINK_LOW`, 0.15) is a dim amber and whose flash clips to a bright
 * yellow.
 */
export const READOUT: Rgb = [2.6, 1.7, 0.35];

/** The unlit buttons: a mid grey. */
const BUTTON_GREY: Rgb = [0.55, 0.56, 0.58];

/** The two lit buttons' plastic, green and red. */
const BUTTON_GREEN: Rgb = [0.2, 0.55, 0.25];
const BUTTON_RED: Rgb = [0.65, 0.14, 0.1];

/** The two lit buttons' lamp caps, above 1 so their low phase is the plastic's own colour. */
const LAMP_GREEN: Rgb = [0.9, 3.2, 1.0];
const LAMP_RED: Rgb = [3.2, 0.75, 0.55];

/** The console's pillar: its top, in metres (waist height). */
const PILLAR_TOP = 0.85;

/**
 * The deck's side profile, `[d, h]`: a 1.5 cm lip at the front, rising
 * 0.1 m to the back edge, where it meets the facia.
 */
const DECK: readonly (readonly [number, number])[] = [
  [0.22, PILLAR_TOP],
  [0.22, 0.865],
  [-0.2, 0.965],
  [-0.2, PILLAR_TOP],
];

/** The deck's top at depth `d`, read off the slant of `DECK`. */
function deckAt(d: number): number {
  const [f, b] = [DECK[1], DECK[2]];
  if (!f || !b) throw new Error("the deck profile");
  return f[1] + ((f[0] - d) / (f[0] - b[0])) * (b[1] - f[1]);
}

/** The facia's front face, in metres along `d`. */
const FACIA_FRONT = -0.2;

/** The console's top, in metres (C3). */
const CONSOLE_TOP = 1.15;

/** The dial: its centre along `a` and in height, and its rim's and face's radii. */
const DIAL = { a: -0.13, h: 1.058, rim: 0.085, face: 0.075 } as const;

/** The needle's length, width and lean from upright, towards `+a`. */
const NEEDLE_LEN = 0.065;
const NEEDLE_W = 0.008;
const NEEDLE_LEAN = (40 * Math.PI) / 180;

/** The screen's window, `a` and `h` ranges, and the readout's margin inside it. */
const SCREEN = { a0: 0.05, a1: 0.27, h0: 0.99, h1: 1.12 } as const;
const SCREEN_MARGIN = 0.015;

/** The buttons: their centres along `a`, their width, depth, front edge and height above the deck. */
const BUTTON_AS = [-0.25, -0.15, -0.05, 0.05, 0.15, 0.25] as const;
const BUTTON_W = 0.07;
const BUTTON_D = 0.05;
const BUTTON_FRONT = 0.1;
const BUTTON_H = 0.035;

/** The pillar's mark: the round C's pixel size and middle, the word's pixel size and middle. */
const PILLAR_LOGO_PX = 0.016;
const PILLAR_LOGO_MID = 0.6;
const PILLAR_WORD_PX = 0.0065;
const PILLAR_WORD_MID = 0.4;

/** A picture's width in metres at pixel size `px`. */
const widthOf = (rows: readonly string[], px: number) =>
  (rows[0]?.length ?? 0) * px;

/**
 * The round C over the word, centred on `a` 0 at depth `d` of kit `k`,
 * facing `+d` in blue: the logo's middle at `logoMid`, the word's at
 * `wordMid`.
 */
function mark(
  k: Kit,
  s: Surfaces,
  d: number,
  logoPx: number,
  logoMid: number,
  wordPx: number,
  wordMid: number,
): void {
  const blue = s.tinted(MARK_BLUE);
  const lit = (ch: string) => (ch === "." ? null : blue);
  const word = textRows(MARKS.capsuleWord);
  const at = (rows: readonly string[], px: number, mid: number) =>
    pixelPanel(
      k,
      rows,
      -widthOf(rows, px) / 2,
      mid + (rows.length * px) / 2,
      px,
      d,
      lit,
    );
  at(CAPSULE_LOGO, logoPx, logoMid);
  at(word, wordPx, wordMid);
}

/**
 * The marked crate standing at `h0`: the pale body, the four corner
 * guards, the eight edge ribs between them and the mark on all four
 * sides.
 */
function markedBox(k: Kit, kitAt: KitAt, s: Surfaces, h0: number): void {
  const { hw, hd } = halfSize("marked-crate", 0);
  const h = CRATE_H;
  const bw = hw - CRATE_INSET;
  const bd = hd - CRATE_INSET;
  const white = s.tinted(CRATE_WHITE);
  const grey = s.tinted(GUARD_GREY);
  k.bevelBox(-bw, bw, -bd, bd, h0, h0 + h - TOP_DROP, 0.02, white);
  for (const sa of [-1, 1] as const)
    for (const sd of [-1, 1] as const)
      k.box(
        sa * hw,
        sa * (hw - GUARD),
        sd * hd,
        sd * (hd - GUARD),
        h0,
        h0 + h,
        grey,
      );
  // The ribs run between the guards, sunk as deep into the body as they
  // stand proud of it.
  const ia = hw - GUARD;
  const id = hd - GUARD;
  for (const [r0, r1] of [
    [h0, h0 + RIB_H],
    [h0 + h - RIB_H, h0 + h],
  ] as const)
    for (const sgn of [-1, 1] as const) {
      k.box(-ia, ia, sgn * (bd - CRATE_INSET), sgn * hd, r0, r1, grey);
      k.box(sgn * (bw - CRATE_INSET), sgn * hw, -id, id, r0, r1, grey);
    }
  const markOn = (kk: Kit, face: number) =>
    mark(
      kk,
      s,
      face + DECAL_LIFT,
      CRATE_LOGO_PX,
      h0 + CRATE_LOGO_TOP - (CAPSULE_LOGO.length * CRATE_LOGO_PX) / 2,
      CRATE_WORD_PX,
      h0 + CRATE_WORD_TOP - (5 * CRATE_WORD_PX) / 2,
    );
  // Each side in a frame turned so its `d` runs out of that side (the
  // front, `+a`, the back, `-a`), so every mark reads the right way round
  // from outside.
  const sides = [
    [0, bd],
    [-Math.PI / 2, bw],
    [Math.PI, bd],
    [Math.PI / 2, bw],
  ] as const;
  for (const [yaw, face] of sides)
    markOn(yaw === 0 ? k : kitAt(yawed(ORIGIN, 0, 0, yaw)), face);
}

/** The marked crate: alone, or on top of a large plain crate. */
const markedCrate: PropRecipe = ({ k, kitAt, s, variant }) => {
  if (variant === 0) {
    markedBox(k, kitAt, s, 0);
    return;
  }
  const { hw, hd } = halfSize("marked-crate", variant);
  k.bevelBox(
    -hw + STACK_INSET,
    hw - STACK_INSET,
    -hd + STACK_INSET,
    hd - STACK_INSET,
    0,
    STACK_BASE_H,
    0.03,
    s.body,
  );
  markedBox(k, kitAt, s, STACK_BASE_H);
};

/** The dial on the facia: rim, face, ticks, needle and hub, each a step nearer the room. */
function dial(k: Kit, s: Surfaces): void {
  const { a, h } = DIAL;
  const dark = s.tinted(DIAL_DARK);
  k.extrude(discOutline(a, h, DIAL.rim, 20), FACIA_FRONT, -0.19, dark);
  k.extrude(
    discOutline(a, h, DIAL.face, 20),
    FACIA_FRONT,
    -0.188,
    s.tinted(PILLAR_WHITE),
  );
  for (let i = 0; i < 12; i++) {
    const t = (2 * Math.PI * i) / 12;
    const long = i % 3 === 0;
    const len = long ? 0.018 : 0.011;
    const r = DIAL.face - 0.006 - len / 2;
    k.extrude(
      tiltedBar(
        a + r * Math.cos(t),
        h + r * Math.sin(t),
        t,
        len,
        long ? 0.006 : 0.004,
      ),
      -0.188,
      -0.186,
      dark,
    );
  }
  const t = Math.PI / 2 - NEEDLE_LEAN;
  k.extrude(
    tiltedBar(
      a + (NEEDLE_LEN / 2) * Math.cos(t),
      h + (NEEDLE_LEN / 2) * Math.sin(t),
      t,
      NEEDLE_LEN,
      NEEDLE_W,
    ),
    -0.188,
    -0.184,
    s.tinted(NEEDLE),
  );
  k.extrude(discOutline(a, h, 0.009, 8), -0.188, -0.182, dark);
}

/** The gravity console: plinth, pillar, slanted deck, facia, dial, screen, buttons and mark. */
const gravityConsole: PropRecipe = ({ k, kitAt, s }) => {
  const white = s.tinted(PILLAR_WHITE);
  const dark = s.tinted(DIAL_DARK);
  const { hw } = halfSize("gravity-console", 0);
  k.box(-0.22, 0.22, -0.17, 0.17, 0, 0.05, s.tinted(PLINTH_DARK));
  k.bevelBox(-0.2, 0.2, -0.15, 0.15, 0.05, PILLAR_TOP, 0.02, white);
  profileAlong(kitAt, ORIGIN, DECK, -hw, hw, s.tinted(DECK_GREY));
  k.box(-hw, hw, -0.25, FACIA_FRONT, PILLAR_TOP, CONSOLE_TOP, white);
  dial(k, s);
  // The screen's dark window and its readout, fitted inside a margin.
  const screenFront = FACIA_FRONT + 0.005;
  k.box(
    SCREEN.a0,
    SCREEN.a1,
    FACIA_FRONT,
    screenFront,
    SCREEN.h0,
    SCREEN.h1,
    dark,
  );
  const rows = textRows(MARKS.gravity);
  const { px, left, top } = fit(
    rows,
    SCREEN.a0 + SCREEN_MARGIN,
    SCREEN.a1 - SCREEN_MARGIN,
    SCREEN.h0 + SCREEN_MARGIN,
    SCREEN.h1 - SCREEN_MARGIN,
  );
  const readout = s.blink(READOUT, 0);
  pixelPanel(k, rows, left, top, px, screenFront + DECAL_LIFT, (ch) =>
    ch === "#" ? readout : null,
  );
  // The buttons, each standing on the slant from the deck's height at its
  // front edge, its back sunk into the rising deck behind it.
  const foot = deckAt(BUTTON_FRONT);
  const back = BUTTON_FRONT - BUTTON_D;
  BUTTON_AS.forEach((a, i) => {
    const [a0, a1] = [a - BUTTON_W / 2, a + BUTTON_W / 2];
    const lamp = i === 1 ? LAMP_GREEN : i === 4 ? LAMP_RED : null;
    const plastic = i === 1 ? BUTTON_GREEN : i === 4 ? BUTTON_RED : BUTTON_GREY;
    k.box(a0, a1, back, BUTTON_FRONT, foot, foot + BUTTON_H, s.tinted(plastic));
    if (lamp === null) return;
    const c = 0.007;
    k.box(
      a0 + c,
      a1 - c,
      back + c,
      BUTTON_FRONT - c,
      foot + BUTTON_H,
      foot + BUTTON_H + 0.003,
      s.blink(lamp, i === 1 ? 1 : 2),
    );
  });
  mark(
    k,
    s,
    0.15 + DECAL_LIFT,
    PILLAR_LOGO_PX,
    PILLAR_LOGO_MID,
    PILLAR_WORD_PX,
    PILLAR_WORD_MID,
  );
};

/** The marked rare props' recipes. */
export const MARKED_RECIPES = {
  "marked-crate": markedCrate,
  "gravity-console": gravityConsole,
} satisfies Record<
  Extract<FloorPropKind, "marked-crate" | "gravity-console">,
  PropRecipe
>;
