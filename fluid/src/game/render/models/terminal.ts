/**
 * The terminal: one `## ` section of the engram, shown on a 70s CRT on an
 * operator's desk, with the chair pulled up in front of it.
 *
 * The desk is a bevelled slab on two pedestals (the right one with
 * drawers), a sloped keyboard deck with four rows of keys sits at its
 * front edge, and a CRT housing stands behind it: a bezel around a
 * recessed screen and a tapering tube case at the back, deeper towards
 * the wall like the real thing. The screen shows the section's text layer
 * and glows; a status LED on the bezel glows beside it. The swivel chair
 * stands as far out as the terminal's footprint allows.
 *
 * `buildTerminal` reads the fixture's variant (absent reads as 0, 2.7 C6)
 * and dispatches to `TERMINAL_RECIPES`, one function per variant, the same
 * shape `machines.ts` and `decor.ts` dispatch their own recipes with.
 * `terminal0` is variant 0, today's model, part for part (2.7 C1); Tasks 4
 * to 7 add the rest.
 */

import { FOOTPRINTS } from "../../world/footprints";
import type { Fixture } from "../../world/types";
import { FLAG } from "../geometry";
import { frameForSlot, type Frame, type Kit } from "../kit";
import { ASPECT } from "../layers";
import {
  profileAlong,
  shade,
  surfaces,
  textPanel,
  type KitAt,
  type ModelContext,
  type Surfaces,
} from "./common";

type Terminal = Extract<Fixture, { kind: "terminal" }>;

/** Half the desk's width, inside the terminal footprint. */
const DESK_HALF = FOOTPRINTS.terminal.along / 2 - 0.02;
/** How deep the desk top is, from the wall. */
const DESK_DEPTH = 0.6;
/** The desk top's underside and top. */
const DESK_H0 = 0.72;
const DESK_H1 = 0.78;
/** The keyboard deck: its front edge and back edge depth and height. */
const DECK_FRONT = 0.58;
const DECK_BACK = 0.36;
const DECK_FRONT_H = 0.8;
const DECK_BACK_H = 0.86;
const DECK_HALF = 0.42;
const KEY_ROWS = 4;
const KEYS_PER_ROW = 12;
const KEY = 0.034;
/** The CRT: screen width, bezel border, the housing's front and back depth. */
const SCREEN_W = 0.48;
const SCREEN_H = SCREEN_W / ASPECT.screen;
const SCREEN_BOTTOM = 0.95;
const BEZEL = 0.06;
const CRT_FRONT = 0.34;
const CRT_BODY = 0.3;
const CRT_BACK = 0.12;
/** The chair: its centre out from the wall and the seat's radius. */
const CHAIR_D = 0.69;
const SEAT_HALF = 0.2;
/** The chair's foot disc radius (`buildTerminal`'s own `k.cylinder` call), the widest part. */
const CHAIR_FOOT_R = 0.21;
/** The backrest's outer depth, `CHAIR_D + 0.2` in `buildTerminal`. */
const CHAIR_BACK_D = CHAIR_D + 0.2;
/** The backrest's own top, in `buildTerminal`'s own numbers. */
const CHAIR_TOP = 0.98;

/** The CRT housing's half-width and top, `buildTerminal`'s own numbers. */
const CRT_HALF_W = SCREEN_W / 2 + BEZEL;
const CRT_TOP = SCREEN_BOTTOM + SCREEN_H + BEZEL;

/**
 * A part's box in the terminal's own local a/d/h frame (`a` along the
 * wall, `d` out from it, `h` up): the same terms `turnedBox` (`world/
 * footprints.ts`) turns into world metres.
 */
export interface TerminalPart {
  a0: number;
  a1: number;
  d0: number;
  d1: number;
  h0: number;
  h1: number;
}

/**
 * The terminal's own solid parts, read from `buildTerminal`'s own box and
 * cylinder calls: the two pedestals (the desk's own support, floor to
 * `DESK_H0`), the keyboard deck, the CRT housing and the swivel chair (one
 * bounding box over its foot, column, seat and back, out to `CHAIR_FOOT_R`
 * and up to `CHAIR_TOP`) - everything the model builds solid, except the
 * desk top itself (already a curio host surface, `world/curios.ts`'s
 * `FIXTURE_SURFACES.terminal`). The chair fills the open knee space
 * between the pedestals, so no standing player can see past it into that
 * space, which is why a terminal carries no under spot. `dev/spots.ts`'s
 * curio-framing sight-line check turns these parts into occluding volumes
 * the way `hostSurfaces` turns a `SurfaceSpec` into a world box, so a
 * chair pulled up to the desk, or the monitor and keyboard on top of it,
 * blocks a framing spot's view exactly as it blocks a player's in the
 * rendered room.
 */
export const TERMINAL_OCCLUDERS: readonly TerminalPart[] = [
  // The left pedestal (the modesty panel's own support).
  {
    a0: -DESK_HALF,
    a1: -DESK_HALF + 0.18,
    d0: 0.02,
    d1: DESK_DEPTH - 0.03,
    h0: 0,
    h1: DESK_H0,
  },
  // The right pedestal, with its drawers.
  {
    a0: DESK_HALF - 0.38,
    a1: DESK_HALF,
    d0: 0.02,
    d1: DESK_DEPTH - 0.03,
    h0: 0,
    h1: DESK_H0,
  },
  // The sloped keyboard deck.
  {
    a0: -DECK_HALF,
    a1: DECK_HALF,
    d0: DECK_BACK,
    d1: DECK_FRONT,
    h0: DECK_BACK_H,
    h1: DECK_FRONT_H,
  },
  // The CRT housing.
  {
    a0: -CRT_HALF_W,
    a1: CRT_HALF_W,
    d0: CRT_BACK,
    d1: CRT_FRONT,
    h0: DESK_H1,
    h1: CRT_TOP,
  },
  // The swivel chair, one box over its foot, column, seat and back.
  {
    a0: -CHAIR_FOOT_R,
    a1: CHAIR_FOOT_R,
    d0: CHAIR_D - CHAIR_FOOT_R,
    d1: CHAIR_BACK_D,
    h0: 0,
    h1: CHAIR_TOP,
  },
];

/** The deck's surface height at depth `d`. */
const deckAt = (d: number) =>
  DECK_FRONT_H +
  ((DECK_FRONT - d) / (DECK_FRONT - DECK_BACK)) * (DECK_BACK_H - DECK_FRONT_H);

/**
 * What a terminal recipe gets: its kit, the frame, the look's surfaces, the
 * room context, the fixture's own index (its text layer's key) and which
 * variant is being drawn.
 */
interface Recipe {
  k: Kit;
  kitAt: KitAt;
  f: Frame;
  s: Surfaces;
  ctx: ModelContext;
  index: number;
  variant: number;
}

/**
 * Builds the terminal against its wall slot, from its variant (absent
 * reads as 0, 2.7 C6).
 */
export function buildTerminal(
  kitAt: KitAt,
  fx: Terminal,
  index: number,
  ctx: ModelContext,
): void {
  const f = frameForSlot(fx.slot);
  const variant = fx.variant ?? 0;
  const recipe = TERMINAL_RECIPES[variant] ?? terminal0;
  recipe({
    k: kitAt(f),
    kitAt,
    f,
    s: surfaces(ctx.look),
    ctx,
    index,
    variant,
  });
}

/** Terminal variant 0: today's desk, CRT and swivel chair, part for part (2.7 C1). */
function terminal0({ k, kitAt, f, s, ctx, index }: Recipe): void {
  const p = ctx.look.palette;

  // The desk: top, two pedestals, a modesty panel and drawer fronts.
  k.bevelBox(
    -DESK_HALF,
    DESK_HALF,
    0,
    DESK_DEPTH,
    DESK_H0,
    DESK_H1,
    0.015,
    s.body,
  );
  k.bevelBox(
    -DESK_HALF,
    -DESK_HALF + 0.18,
    0.02,
    DESK_DEPTH - 0.03,
    0,
    DESK_H0,
    0.02,
    s.body,
  );
  k.bevelBox(
    DESK_HALF - 0.38,
    DESK_HALF,
    0.02,
    DESK_DEPTH - 0.03,
    0,
    DESK_H0,
    0.02,
    s.body,
  );
  k.box(
    -DESK_HALF + 0.18,
    DESK_HALF - 0.38,
    0.02,
    0.05,
    0.25,
    DESK_H0,
    s.metal,
  );
  for (let i = 0; i < 3; i++) {
    const h0 = 0.06 + i * 0.22;
    k.box(
      DESK_HALF - 0.36,
      DESK_HALF - 0.02,
      DESK_DEPTH - 0.03,
      DESK_DEPTH - 0.01,
      h0,
      h0 + 0.19,
      s.metal,
    );
    k.box(
      DESK_HALF - 0.24,
      DESK_HALF - 0.14,
      DESK_DEPTH - 0.01,
      DESK_DEPTH + 0.01,
      h0 + 0.14,
      h0 + 0.16,
      s.dark,
    );
  }

  // The sloped keyboard deck and its keys, row by row up the slope.
  profileAlong(
    kitAt,
    f,
    [
      [DECK_BACK, DESK_H1],
      [DECK_FRONT, DESK_H1],
      [DECK_FRONT, DECK_FRONT_H],
      [DECK_BACK, DECK_BACK_H],
    ],
    -DECK_HALF,
    DECK_HALF,
    s.dark,
  );
  const pitch = (2 * DECK_HALF - 0.06) / KEYS_PER_ROW;
  for (let r = 0; r < KEY_ROWS; r++) {
    const dc = DECK_FRONT - 0.04 - r * 0.048;
    const h = deckAt(dc);
    for (let c = 0; c < KEYS_PER_ROW; c++) {
      const a0 = -DECK_HALF + 0.03 + c * pitch + (pitch - KEY) / 2;
      k.box(
        a0,
        a0 + KEY,
        dc - KEY / 2,
        dc + KEY / 2,
        h - 0.008,
        h + 0.012,
        r === 0 && c > 9 ? s.metal : s.panel,
      );
    }
  }

  // The CRT: a plinth, the tapering tube case, the body and the bezel.
  const sw = SCREEN_W / 2;
  const [s0, s1] = [SCREEN_BOTTOM, SCREEN_BOTTOM + SCREEN_H];
  const [b0, b1] = [s0 - BEZEL, s1 + BEZEL];
  const bw = sw + BEZEL;
  k.bevelBox(-0.16, 0.16, CRT_BACK, CRT_BODY, DESK_H1, b0, 0.01, s.dark);
  k.bevelBox(-bw, bw, CRT_BACK + 0.06, CRT_BODY, b0, b1, 0.03, s.body);
  profileAlong(
    kitAt,
    f,
    [
      [0.02, b0 + 0.08],
      [CRT_BACK + 0.06, b0 + 0.02],
      [CRT_BACK + 0.06, b1 - 0.02],
      [0.02, b1 - 0.1],
    ],
    -bw + 0.08,
    bw - 0.08,
    s.body,
  );
  k.box(-bw, bw, CRT_BODY, CRT_FRONT, b0, s0, s.body);
  k.box(-bw, bw, CRT_BODY, CRT_FRONT, s1, b1, s.body);
  k.box(-bw, -sw, CRT_BODY, CRT_FRONT, s0, s1, s.body);
  k.box(sw, bw, CRT_BODY, CRT_FRONT, s0, s1, s.body);
  textPanel(k, ctx, `terminal:${index}`, -sw, sw, CRT_BODY, s0, s1, {
    tint: [1, 1, 1],
    flag: FLAG.emissive,
  });
  // The status LED and a row of toggle switches under the screen.
  k.box(
    bw - 0.07,
    bw - 0.04,
    CRT_FRONT,
    CRT_FRONT + 0.008,
    b0 + 0.02,
    b0 + 0.04,
    s.glow(p.screenText),
  );
  for (let i = 0; i < 4; i++) {
    const a0 = -bw + 0.05 + i * 0.05;
    k.box(
      a0,
      a0 + 0.015,
      CRT_FRONT,
      CRT_FRONT + 0.02,
      b0 + 0.02,
      b0 + 0.045,
      s.metal,
    );
  }

  // The swivel chair: a five-legged foot, a column, the seat and its back.
  const seat = shade(p.metal, 0.6);
  k.cylinder(0, CHAIR_D, 0, 0.05, 0.21, 10, s.dark);
  k.cylinder(0, CHAIR_D, 0.05, 0.42, 0.03, 8, s.metal);
  k.bevelBox(
    -SEAT_HALF,
    SEAT_HALF,
    CHAIR_D - SEAT_HALF,
    CHAIR_D + SEAT_HALF,
    0.42,
    0.5,
    0.03,
    s.tinted(seat),
  );
  k.box(-0.03, 0.03, CHAIR_D + 0.1, CHAIR_D + 0.16, 0.5, 0.62, s.metal);
  k.bevelBox(
    -0.19,
    0.19,
    CHAIR_D + 0.14,
    CHAIR_D + 0.2,
    0.58,
    0.98,
    0.025,
    s.tinted(seat),
  );
}

/** The recipe of every terminal variant, indexed by `fx.variant ?? 0` (2.7 C1). */
const TERMINAL_RECIPES: readonly ((r: Recipe) => void)[] = [terminal0];
