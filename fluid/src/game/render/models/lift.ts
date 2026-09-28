/**
 * The station's lift, wall screen and exit (M3 C7, C20, C24, C28), each a
 * flush fixture inside its wall slot's band (the slot's cell along, less
 * than `FLUSH_DEPTH` out).
 *
 * - The **lift** is a frame 1.9 m wide and 2.4 m tall: a narrow west jamb,
 *   a lintel over the doors with a lit strip in the room's accent, and a
 *   pier east of the doors that carries the call panel. The panel's
 *   screen shows the `lift:<i>` text layer (the stops, `LIFT_LINES` of
 *   them, the overflow line and the note), with two round glowing buttons
 *   under it, an up and a down arrow in relief. The two brushed-metal
 *   leaves stand 0.08 m into the frame and are movers under the door key
 *   (`door:<i>`), which the lift's door state drives: at `open` 1 both
 *   have slid east into the pier, the far one twice as far as the near
 *   one at a depth of its own, as a two-speed lift door opens. A
 *   centre-parting pair cannot open here: the west jamb is 0.15 m wide,
 *   and a leaf parked west of the doors would leave the wall band.
 * - The **screen** is a dark bezel 1.6 by 0.9 m, its bottom at 1.4 m,
 *   around glass in the text layer's own background colour, with the
 *   `screen:<i>` text quad (kind `station`, `SCREEN_LINES` rows) as tall
 *   as the glass less a 0.02 m margin and centred on it, and one green
 *   status lamp under it.
 * - The **exit** is the sliding door (`slidingDoor`) with its label drawn
 *   under `exit:<i>` and a small plate on its west jamb showing the
 *   `door` pictogram; its leaves, lamp and sparks keep the door keys.
 */

import type { Fixture } from "../../world/types";
import { FLAG, accentTint, type Surface, type V3 } from "../geometry";
import { DECAL_LIFT, frameForSlot } from "../kit";
import { ASPECT, LAYER } from "../layers";
import { PICTOGRAM, colours } from "../text";
import {
  RECESS,
  discOutline,
  surfaces,
  textPanel,
  type KitAt,
  type ModelContext,
  type Mover,
} from "./common";
import { leaves, slidingDoor } from "./doors";

type Lift = Extract<Fixture, { kind: "lift" }>;
type Screen = Extract<Fixture, { kind: "screen" }>;
type Exit = Extract<Fixture, { kind: "exit" }>;

/** The lift's frame: its half width, its top and the depth of its face. */
const FRAME_HALF = 0.95;
const FRAME_TOP = 2.4;
const FRAME_D = 0.2;
/** How far the leaves' faces stand behind the frame's face. */
const DOOR_SET = 0.08;

/**
 * The lift's doorway in its frame, along the wall and up from the floor:
 * `a0` to `a1` along (the west jamb's inner face to the pier's), `h0` the
 * sill's top to `h1` the lintel's underside. At `open` 1 no part of a
 * leaf is left inside it (the models test checks it).
 */
export const LIFT_OPENING = { a0: -0.8, a1: 0.3, h0: 0.02, h1: 2.2 } as const;

/**
 * The pier the leaves park in at `open` 1: from the doorway's east edge
 * `a0` to the frame's `a1`, closed in front by its face at depth `front`.
 * A parked leaf lies wholly inside it, hidden.
 */
export const LIFT_POCKET = { a0: 0.3, a1: FRAME_HALF, front: FRAME_D } as const;

/** How far a shut leaf reaches past the doorway into the jamb or pier. */
const TUCK = 0.02;
/**
 * How far past the pier's west face a parked leaf's west edge stops, so
 * the leaf's end never lies in the plane of that face.
 */
const PARK_GAP = 0.01;
/**
 * The two leaves, `[a0, a1, d0, d1]` shut: the far (west) leaf in front,
 * its face `DOOR_SET` behind the frame's, overlapping the near leaf at the
 * meeting line, and the near (east) leaf behind it. Each slides east until
 * its west edge is `PARK_GAP` inside the pier.
 */
const FAR_LEAF = [
  LIFT_OPENING.a0 - TUCK,
  -0.24,
  FRAME_D - DOOR_SET - 0.04,
  FRAME_D - DOOR_SET,
] as const;
const NEAR_LEAF = [-0.26, LIFT_OPENING.a1 + TUCK, 0.03, 0.07] as const;

/** The lit strip over the doors: along, heights and how far it stands out. */
const STRIP = { a0: -0.75, a1: 0.25, h0: 2.28, h1: 2.32, d: 0.015 } as const;

/** The call panel on the pier: along, heights and its face's depth. */
const PANEL = { a0: 0.5, a1: 0.8, h0: 1.1, h1: 1.65, d: FRAME_D + 0.035 };
/** The panel's screen: its width and its top. */
const PANEL_SCREEN_W = 0.26;
const PANEL_SCREEN_TOP = 1.63;
/** The two call buttons: their height, radius, centres and depth. */
const BUTTON_H = 1.19;
const BUTTON_R = 0.042;
const BUTTON_A = [0.595, 0.705] as const;
const BUTTON_D = 0.01;
/** The buttons' glow: a warm white, the same in every look. */
const BUTTON_TINT: V3 = [1, 0.9, 0.7];

/**
 * The lift: frame, lit strip, call panel and its two leaves, returned as
 * movers (far leaf, then near leaf) under `door:<index>`.
 */
export function buildLift(
  kitAt: KitAt,
  fx: Lift,
  index: number,
  ctx: ModelContext,
): Mover[] {
  const f = frameForSlot(fx.slot);
  const k = kitAt(f);
  const s = surfaces(ctx.look);
  const o = LIFT_OPENING;

  // The dark shaft behind the leaves, the sill, the jamb, the lintel and
  // the pier the leaves slide into.
  k.panel(o.a0, o.a1, DECAL_LIFT, o.h0, o.h1, {
    layer: LAYER.panel,
    tint: RECESS,
    flag: FLAG.lit,
  });
  k.box(o.a0, o.a1, 0, FRAME_D, 0, o.h0, s.dark);
  k.bevelBox(-FRAME_HALF, o.a0, 0, FRAME_D, 0, FRAME_TOP, 0.015, s.body);
  k.bevelBox(o.a0, o.a1, 0, FRAME_D, o.h1, FRAME_TOP, 0.015, s.body);
  k.bevelBox(o.a1, FRAME_HALF, 0, FRAME_D, 0, FRAME_TOP, 0.015, s.body);
  // The lit strip over the doors, in the room's accent (steady).
  const strip: Surface = {
    layer: LAYER.panel,
    tint: accentTint(1),
    flag: FLAG.emissive,
  };
  k.box(
    STRIP.a0,
    STRIP.a1,
    FRAME_D,
    FRAME_D + STRIP.d,
    STRIP.h0,
    STRIP.h1,
    strip,
  );

  // The call panel: a dark plate, its screen, and the two buttons.
  k.bevelBox(
    PANEL.a0,
    PANEL.a1,
    FRAME_D,
    PANEL.d,
    PANEL.h0,
    PANEL.h1,
    0.008,
    s.dark,
  );
  const mid = (PANEL.a0 + PANEL.a1) / 2;
  const screenH = PANEL_SCREEN_W / ASPECT.panel;
  textPanel(
    k,
    ctx,
    `lift:${index}`,
    mid - PANEL_SCREEN_W / 2,
    mid + PANEL_SCREEN_W / 2,
    PANEL.d,
    PANEL_SCREEN_TOP - screenH,
    PANEL_SCREEN_TOP,
    { tint: [1, 1, 1], flag: FLAG.emissive },
  );
  const glow = s.glow(BUTTON_TINT);
  BUTTON_A.forEach((a, i) => {
    k.extrude(
      discOutline(a, BUTTON_H, BUTTON_R, 16),
      PANEL.d,
      PANEL.d + BUTTON_D,
      glow,
    );
    // The arrow in relief: up on the first button, down on the second.
    const dir = i === 0 ? 1 : -1;
    const [w, t] = [0.02, 0.016];
    k.extrude(
      [
        [a - w, BUTTON_H - dir * t * 0.7],
        [a + w, BUTTON_H - dir * t * 0.7],
        [a, BUTTON_H + dir * t],
      ],
      PANEL.d + BUTTON_D,
      PANEL.d + BUTTON_D + 0.004,
      s.dark,
    );
  });

  // The leaves, brushed metal, as the doors' leaf movers under the door
  // key: each slides east until its west edge is just inside the pier,
  // the far one twice as far as the near one.
  const out = leaves(f, `door:${index}`, index);
  for (const [a0, a1, d0, d1] of [FAR_LEAF, NEAR_LEAF]) {
    out.add([...f.along], LIFT_POCKET.a0 + PARK_GAP - a0, (m) => {
      m.bevelBox(a0, a1, d0, d1, o.h0, o.h1 + TUCK / 2, 0.006, s.metal);
    });
  }
  return out.movers;
}

/** The wall screen's bezel: half width, bottom, top and depth. */
const SCREEN_HALF = 0.8;
const SCREEN_BOTTOM = 1.4;
const SCREEN_TOP = 2.3;
const BEZEL_D = 0.06;
/** How wide the bezel's rim is round the glass. */
const BEZEL_RIM = 0.04;

/**
 * The wall screen's glass inside the bezel: `half` its half width along
 * the wall, `h0` to `h1` its height. The text quad spans it less
 * `TEXT_MARGIN` at the top and bottom, at `ASPECT.station`.
 */
export const SCREEN_GLASS = {
  half: SCREEN_HALF - BEZEL_RIM,
  h0: SCREEN_BOTTOM + BEZEL_RIM,
  h1: SCREEN_TOP - BEZEL_RIM,
} as const;
/**
 * The patch of the panel texture the glass shows: `size` square from
 * `(at, at)`, inside one of the texture's four tiles and clear of the
 * bevel lines at 0 and 0.5, which would otherwise draw a line across the
 * middle of the glass.
 */
const GLASS_UV = { at: 0.1, size: 0.3 } as const;
/** The text quad's margin inside the glass, top and bottom. */
const TEXT_MARGIN = 0.02;
/** The status lamp under the screen: its centre along, height and size. */
const LAMP_A = 0.7;
const LAMP_H = 1.33;
const LAMP_SIZE = 0.04;
/** The status lamp's colour: a steady green. */
const LAMP_GREEN: V3 = [0.35, 1, 0.45];

/**
 * The wall screen: the bezel, the glass in the station text's background
 * colour, the `screen:<index>` text quad as tall as the glass less
 * `TEXT_MARGIN` top and bottom and centred on it, its width taken from
 * `ASPECT.station` so the quad and the text layer cannot drift apart
 * (the glass either side reads as the same screen), and one status lamp
 * on a small mount under it.
 */
export function buildScreen(
  kitAt: KitAt,
  fx: Screen,
  index: number,
  ctx: ModelContext,
): Mover[] {
  const k = kitAt(frameForSlot(fx.slot));
  const s = surfaces(ctx.look);
  k.bevelBox(
    -SCREEN_HALF,
    SCREEN_HALF,
    0,
    BEZEL_D,
    SCREEN_BOTTOM,
    SCREEN_TOP,
    0.012,
    s.dark,
  );
  // The glass samples one plain patch of the panel texture
  // (`GLASS_UV`), clear of its bevel lines, so no seam crosses the glass.
  const { half: glassHalf, h0: g0, h1: g1 } = SCREEN_GLASS;
  k.panel(
    -glassHalf,
    glassHalf,
    BEZEL_D + DECAL_LIFT,
    g0,
    g1,
    s.glow(colours("station", ctx.look).background),
    GLASS_UV.size,
    GLASS_UV.size,
    GLASS_UV.at,
    GLASS_UV.at,
  );
  const h = g1 - g0 - 2 * TEXT_MARGIN;
  const half = (h * ASPECT.station) / 2;
  textPanel(
    k,
    ctx,
    `screen:${index}`,
    -half,
    half,
    BEZEL_D + DECAL_LIFT,
    g0 + TEXT_MARGIN,
    g1 - TEXT_MARGIN,
    { tint: [1, 1, 1], flag: FLAG.emissive },
  );
  // The status lamp on its mount.
  const l = LAMP_SIZE / 2;
  k.box(
    LAMP_A - l - 0.015,
    LAMP_A + l + 0.015,
    0,
    0.03,
    LAMP_H - l - 0.015,
    LAMP_H + l + 0.015,
    s.dark,
  );
  k.panel(
    LAMP_A - l,
    LAMP_A + l,
    0.03 + DECAL_LIFT,
    LAMP_H - l,
    LAMP_H + l,
    s.signal(LAMP_GREEN),
  );
  return [];
}

/** The exit's pictogram plate on the west jamb: along, heights, depth. */
const PLATE = { a0: -0.86, a1: -0.64, h0: 1.45, h1: 1.67, d: 0.015 } as const;
/** The jamb face the plate sits on (the sliding door's frame depth). */
const JAMB_D = 0.12;

/**
 * The exit: the sliding door with its label under `exit:<index>` (M3
 * C28), and a small dark plate on its west jamb carrying the `door`
 * pictogram, the way out.
 */
export function buildExit(
  kitAt: KitAt,
  fx: Exit,
  index: number,
  ctx: ModelContext,
): Mover[] {
  const movers = slidingDoor(kitAt, fx.slot, index, ctx, `exit:${index}`);
  const k = kitAt(frameForSlot(fx.slot));
  const s = surfaces(ctx.look);
  const p = PLATE;
  k.box(p.a0, p.a1, JAMB_D, JAMB_D + p.d, p.h0, p.h1, s.dark);
  const u = PICTOGRAM.door;
  const inset = 0.015;
  k.panel(
    p.a0 + inset,
    p.a1 - inset,
    JAMB_D + p.d + DECAL_LIFT,
    p.h0 + inset,
    p.h1 - inset,
    { layer: LAYER.pictogram, tint: [1, 1, 1], flag: FLAG.emissive },
    u.uw,
    u.vh,
    u.u0,
    u.v0,
  );
  return movers;
}
