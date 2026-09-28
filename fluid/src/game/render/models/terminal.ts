/**
 * The terminal: one `## ` section of the engram, shown on a screen on an
 * operator's desk, with a seat pulled up in front of it.
 *
 * `buildTerminal` reads the fixture's variant (absent reads as 0, 2.7 C6)
 * and dispatches to `TERMINAL_RECIPES`, one function per variant, the same
 * shape `machines.ts` and `decor.ts` dispatch their own recipes with:
 *
 * - **Variant 0** (`terminal0`), a 70s CRT desk. A sloped keyboard deck
 *   with four rows of keys sits at the desk's front edge and a CRT housing
 *   stands behind it: a bezel around a recessed screen and a tapering tube
 *   case at the back, deeper towards the wall like the real thing. A
 *   status LED on the bezel glows beside the screen. The swivel chair
 *   stands as far out as the terminal's footprint allows.
 * - **Variant 1** (`flatConsole`), the flat console: a thin flat monitor on
 *   a short stand round the same screen, a flat keyboard on a tray under
 *   the desk's front edge and a round backless stool.
 * - **Variant 2** (`hoodedTwin`), the hooded twin: variant 0's CRT under a
 *   sun hood, a small side monitor on an arm to its left showing steady
 *   green bars (a light, never text), a board on the wall behind both
 *   screens and an arm chair.
 *
 * Every variant keeps the terminal's envelope (2.7 C3): the desk top (both
 * ends are curio host surfaces), both pedestals and the knee space between
 * them, and the screen quad itself (`screen`: one call for every variant,
 * the same rectangle, depth and `ASPECT.screen`), so the section's text
 * and its reading never move with the housing round it. `TERMINAL_OCCLUDERS`
 * is variant 0's table and stands for every variant. The room's accent
 * (2.7 C9) goes on the seat (and the chair's back where it has one) and on
 * the three drawer pulls, in every variant.
 */

import { FOOTPRINTS } from "../../world/footprints";
import type { Fixture } from "../../world/types";
import { FLAG, accentTint, type Surface } from "../geometry";
import { DECAL_LIFT, frameForSlot, type Frame, type Kit } from "../kit";
import { ASPECT } from "../layers";
import { hueToRgb } from "../looks";
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
/** The chair's foot disc radius (`swivelChair`'s own `k.cylinder` call), the widest part. */
const CHAIR_FOOT_R = 0.21;
/** The backrest's outer depth, `CHAIR_D + 0.2` in `swivelChair`. */
const CHAIR_BACK_D = CHAIR_D + 0.2;
/** The backrest's own top, in `swivelChair`'s own numbers. */
const CHAIR_TOP = 0.98;

/** The CRT housing's half-width and top, `crt`'s own numbers. */
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
 * The terminal's own solid parts, variant 0's (`terminal0`'s own box and
 * cylinder calls), standing for every variant (2.7 C3): the two pedestals (the desk's own support, floor to
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

/**
 * A drawer pull in the room's accent (2.7 C9): the pull's own gunmetal
 * surface with the accent mark for its tint, so on variant 0 the pulls are
 * re-tinted and keep their layer and flag.
 */
const pullSurface = (s: Surfaces): Surface => ({
  ...s.dark,
  tint: accentTint(1),
});

/**
 * The desk every variant keeps (2.7 C3): the top (the curio host surface
 * at both ends), two pedestals with the knee space between them, a modesty
 * panel and the right pedestal's three drawers with their accent pulls.
 */
function desk({ k, s }: Recipe): void {
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
  const pull = pullSurface(s);
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
      pull,
    );
  }
}

/** The sloped keyboard deck at the desk's front and its keys, row by row up the slope. */
function keyDeck({ k, kitAt, f, s }: Recipe): void {
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
}

/**
 * The screen quad every variant draws (2.7 C3): the section's text layer,
 * `SCREEN_W` wide from `SCREEN_BOTTOM`, `ASPECT.screen` tall, lifted off a
 * face at `CRT_BODY`, glowing. One call for every variant, so the text and
 * the reading never move with the housing around it.
 */
function screen({ k, ctx, index }: Recipe): void {
  const sw = SCREEN_W / 2;
  textPanel(
    k,
    ctx,
    `terminal:${index}`,
    -sw,
    sw,
    CRT_BODY,
    SCREEN_BOTTOM,
    SCREEN_BOTTOM + SCREEN_H,
    { tint: [1, 1, 1], flag: FLAG.emissive },
  );
}

/**
 * The CRT: a plinth, the tapering tube case back to the wall, the body and
 * the bezel around the recessed screen, then the status LED and a row of
 * toggle switches under the screen.
 */
function crt(r: Recipe): void {
  const { k, kitAt, f, s, ctx } = r;
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
  screen(r);
  k.box(
    bw - 0.07,
    bw - 0.04,
    CRT_FRONT,
    CRT_FRONT + 0.008,
    b0 + 0.02,
    b0 + 0.04,
    s.glow(ctx.look.palette.screenText),
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
}

/**
 * The swivel chair: a five-legged foot, a column, the seat and its back,
 * the seat and back in the room's accent (2.7 C9).
 */
function swivelChair({ k, s }: Recipe): void {
  k.cylinder(0, CHAIR_D, 0, 0.05, CHAIR_FOOT_R, 10, s.dark);
  k.cylinder(0, CHAIR_D, 0.05, 0.42, 0.03, 8, s.metal);
  k.bevelBox(
    -SEAT_HALF,
    SEAT_HALF,
    CHAIR_D - SEAT_HALF,
    CHAIR_D + SEAT_HALF,
    0.42,
    0.5,
    0.03,
    s.accent(),
  );
  k.box(-0.03, 0.03, CHAIR_D + 0.1, CHAIR_D + 0.16, 0.5, 0.62, s.metal);
  k.bevelBox(
    -0.19,
    0.19,
    CHAIR_D + 0.14,
    CHAIR_D + 0.2,
    0.58,
    CHAIR_TOP,
    0.025,
    s.accent(),
  );
}

/** Terminal variant 0: today's desk, CRT and swivel chair, part for part (2.7 C1). */
function terminal0(r: Recipe): void {
  desk(r);
  keyDeck(r);
  crt(r);
  swivelChair(r);
}

/**
 * The flat console (variant 1): a thin flat monitor on a short stand, a
 * flat keyboard on a tray under the desk's front edge and a round backless
 * stool.
 */
const FLAT = {
  /** The bezel's width round the screen, and how far it stands proud. */
  bezel: 0.03,
  proud: 0.012,
  /** The housing's back: 0.1 m behind the screen's face, no tube case. */
  back: CRT_BODY - 0.1,
  /** The stand's neck (half-width, depth) and its foot (half-width, depth). */
  neck: { half: 0.035, d: [0.215, 0.255] },
  foot: { half: 0.14, d: [0.12, 0.34], h: 0.015 },
  /** The keyboard tray under the desk's front edge, on two runners. */
  tray: { half: 0.26, d: [0.28, 0.66], h: [0.64, 0.655] },
  runner: 0.02,
  keyboard: { half: 0.22, d: [0.42, 0.6], h: 0.02 },
  /** The stool's seat radius, its top and its foot ring. */
  seat: { r: 0.17, h: [0.47, 0.53] },
  ring: { r: 0.15, h: 0.2, tube: 0.012 },
} as const;

/**
 * Terminal variant 1, the flat console: the desk, then a thin flat monitor
 * housing round the same screen quad on a neck and a foot plate (nothing
 * deeper than 0.1 m behind the screen's face), a flat keyboard on a tray
 * hung from two runners under the desk's front edge, and a round backless
 * stool in the knee space, its seat in the room's accent.
 */
function flatConsole(r: Recipe): void {
  const { k, s, ctx } = r;
  const F = FLAT;
  desk(r);

  // The monitor: a flat housing whose face is the screen's backing, a thin
  // bezel frame round the screen, a status LED on the bezel.
  const sw = SCREEN_W / 2;
  const [s0, s1] = [SCREEN_BOTTOM, SCREEN_BOTTOM + SCREEN_H];
  const bw = sw + F.bezel;
  k.bevelBox(
    -bw,
    bw,
    F.back,
    CRT_BODY,
    s0 - F.bezel,
    s1 + F.bezel,
    0.01,
    s.dark,
  );
  const [fd0, fd1] = [CRT_BODY, CRT_BODY + F.proud];
  k.box(-bw, bw, fd0, fd1, s0 - F.bezel, s0, s.dark);
  k.box(-bw, bw, fd0, fd1, s1, s1 + F.bezel, s.dark);
  k.box(-bw, -sw, fd0, fd1, s0, s1, s.dark);
  k.box(sw, bw, fd0, fd1, s0, s1, s.dark);
  screen(r);
  k.box(
    sw - 0.04,
    sw - 0.02,
    fd1,
    fd1 + 0.006,
    s0 - 0.02,
    s0 - 0.01,
    s.glow(ctx.look.palette.screenText),
  );
  // The short stand: a neck under the housing and a foot on the desk.
  const [nd0, nd1] = F.neck.d;
  k.box(-F.neck.half, F.neck.half, nd0, nd1, DESK_H1, s0 - F.bezel, s.metal);
  const [od0, od1] = F.foot.d;
  k.bevelBox(
    -F.foot.half,
    F.foot.half,
    od0,
    od1,
    DESK_H1,
    DESK_H1 + F.foot.h,
    0.005,
    s.metal,
  );

  // The tray and its runners under the desk top, and the flat keyboard.
  const [td0, td1] = F.tray.d;
  const [th0, th1] = F.tray.h;
  for (const side of [-1, 1]) {
    const a = side * (F.tray.half + F.runner / 2);
    k.box(
      a - F.runner / 2,
      a + F.runner / 2,
      0.1,
      DESK_DEPTH - 0.02,
      th0,
      DESK_H0,
      s.metal,
    );
  }
  k.box(-F.tray.half, F.tray.half, td0, td1, th0, th1, s.dark);
  const [kd0, kd1] = F.keyboard.d;
  const kh = th1 + F.keyboard.h;
  k.bevelBox(
    -F.keyboard.half,
    F.keyboard.half,
    kd0,
    kd1,
    th1,
    kh,
    0.006,
    s.body,
  );
  const pitch = (2 * F.keyboard.half - 0.04) / KEYS_PER_ROW;
  for (let row = 0; row < KEY_ROWS; row++) {
    const dc = kd1 - 0.03 - row * 0.04;
    for (let c = 0; c < KEYS_PER_ROW; c++) {
      const a0 = -F.keyboard.half + 0.02 + c * pitch + (pitch - KEY) / 2;
      k.box(a0, a0 + KEY, dc - KEY / 2, dc + KEY / 2, kh, kh + 0.008, s.panel);
    }
  }

  // The round backless stool: a foot, a column, a foot ring on three
  // spokes and the seat.
  k.cylinder(0, CHAIR_D, 0, 0.04, CHAIR_FOOT_R - 0.02, 10, s.dark);
  k.cylinder(0, CHAIR_D, 0.04, F.seat.h[0], 0.028, 8, s.metal);
  k.ring(0, CHAIR_D, F.ring.h, F.ring.r, F.ring.tube, 6, 16, s.metal, "up");
  for (const side of [-1, 1]) {
    const a0 = side < 0 ? -F.ring.r : 0.028;
    const a1 = side < 0 ? -0.028 : F.ring.r;
    k.box(
      a0,
      a1,
      CHAIR_D - 0.008,
      CHAIR_D + 0.008,
      F.ring.h - 0.008,
      F.ring.h + 0.008,
      s.metal,
    );
  }
  k.box(
    -0.008,
    0.008,
    CHAIR_D + 0.028,
    CHAIR_D + F.ring.r,
    F.ring.h - 0.008,
    F.ring.h + 0.008,
    s.metal,
  );
  k.cylinder(0, CHAIR_D, F.seat.h[0], F.seat.h[1], F.seat.r, 16, s.accent());
}

/**
 * The hooded twin (variant 2): a sun hood over the CRT's screen, a small
 * side monitor on an arm to the screen's left, the board on the wall both
 * are mounted on, and an arm chair.
 */
const HOODED = {
  /** How far the hood reaches in front of the bezel, and its thickness. */
  reach: 0.2,
  plate: 0.015,
  /** How much the hood's front edge rises over its back. */
  rise: 0.05,
  /** How far the cheeks reach out at the bezel's foot. */
  foot: 0.05,
  /** The side monitor: along the wall, height, and its housing's depth. */
  side: {
    a: [-0.435, -0.32],
    h: [1.02, 1.2],
    d: [CRT_FRONT - 0.05, CRT_FRONT],
  },
  /** The side monitor's bars: their lengths as shares of its screen's width. */
  bars: [0.9, 0.55, 0.75, 0.35],
  /** The arm out to the housing, and its wall plate. */
  arm: { a: [-0.39, -0.365], h: [1.1, 1.125] },
  wall: { a: [-0.405, -0.35], h: [1.05, 1.17] },
  /** The equipment board both screens are mounted on, flat on the wall. */
  board: { half: 0.44, h: [DESK_H1, 1.62], d: 0.015 },
  /**
   * The arm chair: the seat's half-width, the back's half-width and top.
   * The back stays low, like variant 0's, so it never hides the screen
   * from a player standing back from the desk.
   */
  seat: 0.22,
  back: { half: 0.22, top: 1.0 },
  /** The arm rests: a solid side under a pad, each side of the seat. */
  rest: { a: [0.225, 0.245], pad: [0.215, 0.255], h: [0.63, 0.665] },
  /** The floor runners the sides stand on. */
  runner: 0.04,
} as const;

/**
 * Terminal variant 2, the hooded twin: variant 0's desk, key deck and CRT
 * with a sun hood over the screen (a plate rising to the front and two
 * cheeks tapering down the bezel's sides; the plate's underside stays
 * above the sight line from the use point, so the whole screen stays in
 * view), a metal board flat on the wall behind both screens, a small side
 * monitor on an arm from the board to the screen's left showing a few
 * steady green bars (a signal light, no text) and an arm chair with solid
 * sides on floor runners, its seat and back in the room's accent.
 */
function hoodedTwin(r: Recipe): void {
  const { k, kitAt, f, s } = r;
  const H = HOODED;
  desk(r);
  keyDeck(r);
  crt(r);

  // The hood: a plate over the housing, rising to its front edge, and two
  // cheeks down the bezel's sides.
  const [s0, s1] = [SCREEN_BOTTOM, SCREEN_BOTTOM + SCREEN_H];
  const [b0, b1] = [s0 - BEZEL, s1 + BEZEL];
  const bw = SCREEN_W / 2 + BEZEL;
  const front = CRT_FRONT + H.reach;
  profileAlong(
    kitAt,
    f,
    [
      [CRT_BACK + 0.06, b1],
      [front, b1 + H.rise],
      [front, b1 + H.rise + H.plate],
      [CRT_BACK + 0.06, b1 + H.plate],
    ],
    -bw - H.plate,
    bw + H.plate,
    s.body,
  );
  for (const [a0, a1] of [
    [-bw - H.plate, -bw],
    [bw, bw + H.plate],
  ] as const) {
    profileAlong(
      kitAt,
      f,
      [
        [CRT_FRONT, b0],
        [CRT_FRONT + H.foot, b0],
        [front, b1 + H.rise],
        [CRT_FRONT, b1],
      ],
      a0,
      a1,
      s.body,
    );
  }

  // The board on the wall behind both screens, and the side monitor on
  // its arm from the board, the monitor's face flush with the bezel's.
  const [ma0, ma1] = H.side.a;
  const [mh0, mh1] = H.side.h;
  const [md0, md1] = H.side.d;
  const bd = H.board.d;
  k.box(
    -H.board.half,
    H.board.half,
    0,
    bd,
    H.board.h[0],
    H.board.h[1],
    s.metal,
  );
  k.box(
    H.wall.a[0],
    H.wall.a[1],
    bd,
    bd + 0.012,
    H.wall.h[0],
    H.wall.h[1],
    s.dark,
  );
  k.box(
    H.arm.a[0],
    H.arm.a[1],
    bd + 0.012,
    md0,
    H.arm.h[0],
    H.arm.h[1],
    s.metal,
  );
  k.bevelBox(ma0, ma1, md0, md1, mh0, mh1, 0.008, s.body);
  const inset = 0.012;
  const [ga0, ga1] = [ma0 + inset, ma1 - inset];
  const [gh0, gh1] = [mh0 + inset, mh1 - inset];
  const green = hueToRgb(130, 0.85, 0.55);
  k.panel(ga0, ga1, md1 + DECAL_LIFT, gh0, gh1, s.glow(shade(green, 0.18)));
  const pitch = (gh1 - gh0) / H.bars.length;
  H.bars.forEach((share, i) => {
    const h0 = gh0 + i * pitch + pitch * 0.25;
    k.panel(
      ga0 + 0.01,
      ga0 + 0.01 + share * (ga1 - ga0 - 0.02),
      md1 + 2 * DECAL_LIFT,
      h0,
      h0 + pitch * 0.5,
      s.signal(green),
    );
  });

  // The arm chair: two solid sides on floor runners, each topped by an arm
  // rest's pad, the wide seat between them and a high back on the seat.
  const [rh0, rh1] = H.rest.h;
  const [cd0, cd1] = [CHAIR_D - SEAT_HALF, CHAIR_D + SEAT_HALF - 0.02];
  for (const side of [-1, 1]) {
    const span = (lo: number, hi: number) =>
      side < 0 ? ([-hi, -lo] as const) : ([lo, hi] as const);
    const [pa0, pa1] = span(H.rest.a[0], H.rest.a[1]);
    k.box(pa0, pa1, cd0, cd1, 0, H.runner, s.dark);
    k.box(pa0, pa1, cd0 + 0.06, cd1 - 0.02, H.runner, rh0, s.dark);
    const [qa0, qa1] = span(H.rest.pad[0], H.rest.pad[1]);
    k.bevelBox(qa0, qa1, cd0 + 0.04, cd1 - 0.02, rh0, rh1, 0.01, s.dark);
  }
  k.bevelBox(-H.seat, H.seat, cd0, cd1, 0.42, 0.5, 0.03, s.accent());
  k.bevelBox(
    -H.back.half,
    H.back.half,
    CHAIR_D + 0.13,
    CHAIR_D + 0.2,
    0.5,
    H.back.top,
    0.025,
    s.accent(),
  );
}

/** The recipe of every terminal variant, indexed by `fx.variant ?? 0` (2.7 C1). */
const TERMINAL_RECIPES: readonly ((r: Recipe) => void)[] = [
  terminal0,
  flatConsole,
  hoodedTwin,
];
