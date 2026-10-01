/**
 * The station's look, as a parameter set over every room.
 *
 * A look changes nothing about what is in a room, only how it is drawn: the
 * palette, how bright the light is and how far it carries, how much grime,
 * how strong the neon edge lines are and where they appear, and the five
 * accents a room is picked out in (2.7 C7). The renderer reads nothing else,
 * so the look costs a few dozen numbers.
 *
 * White panels with glowing seams on the room's shell and its ways through,
 * doors in cyan and portals in orange. Props, heroes, fittings, decor,
 * terminals and machines are plain shaded.
 */

import type { Condition, Finish } from "../world/types";

/**
 * A colour, each channel in [0, 1]. The one exception is a surface's
 * accent mark (`accentTint` in `geometry.ts`), a negative first channel the
 * vertex shader swaps for the room's accent.
 */
export type Rgb = readonly [number, number, number];

/** Every parameter the renderer reads from a look. */
export interface Look {
  palette: {
    panel: Rgb;
    floor: Rgb;
    ceiling: Rgb;
    metal: Rgb;
    door: Rgb;
    portal: Rgb;
    /** A portal into another domain. */
    portalAlt: Rgb;
    machine: Rgb;
    screen: Rgb;
    screenText: Rgb;
    lamp: Rgb;
    fog: Rgb;
  };
  /**
   * The body and panel colours of the set dressing, the machines, the
   * terminals and the furniture (`propLook`), so they stand off white walls
   * as light grey console plastic. Absent keeps the palette's `machine` and
   * `panel` for them too. The shell and its ways (doors, portals, hatches,
   * lifts, screens), the heroes and the curios never take it.
   */
  propBody?: { body: Rgb; panel: Rgb };
  /** Multiplies every light level. */
  lightScale: number;
  /** How quickly light falls off with distance (the classic banded distance light). */
  falloff: number;
  /** The darkest a lit surface gets, 0 to 1. */
  minLight: number;
  /** How many brightness bands the falloff is quantised to (classically 32). */
  bands: number;
  /** 0 clean to 1 filthy. */
  grime: number;
  /**
   * How far the contact shadows under things (`shadows.ts`) darken the
   * floor's light at their darkest, 0 to 1. Absent draws none.
   */
  contactShadow?: number;
  edge: {
    colour: Rgb;
    /** Added brightness of an edge line; above 1 feeds the bloom. */
    strength: number;
    /**
     * Seams on the room's shell, not only on frames: on every surface
     * flagged `shell` (`FLAG.shell`, `buildRoomMesh`), never on a prop, a
     * hero, a fitting or other furniture.
     */
    everywhere: boolean;
    /** Line width in pixels. */
    width: number;
    /**
     * How the seams on the shell fade with distance, in metres from the
     * eye: at full strength up to `from`, gone from `to` on, a smooth step
     * between. It keeps a long corridor's far end, where the seams crowd
     * into a few pixels, from washing out to white; the walls keep their
     * colour, only the lines fade, and the near view is unchanged. Null
     * for a look without seams everywhere. Frames never fade.
     */
    fade: { from: number; to: number } | null;
  };
  bloom: { threshold: number; strength: number };
  /**
   * The look's accent set (2.7 C7): `ACCENT_COUNT` colours (0 yellow, 1
   * red, 2 green, 3 violet, 4 teal), each at least 0.25 (RGB distance)
   * from the look's `door`, `portal` and `portalAlt`, so an accent never
   * reads as a way. A room picks one by index (`Finish.accent`) and
   * `accentFor` reads it.
   */
  accents: readonly Rgb[];
  /**
   * True when each prop of the set dressing paints one part (a lid, a band,
   * a handle, one door) in its own accent from `accents`, picked by its
   * seed (`propAccentPick`), and its other accent parts in dark metal.
   * Absent, a prop's accent parts take the room's accent.
   */
  propAccents?: boolean;
}

function hex(value: number): Rgb {
  return [
    ((value >> 16) & 255) / 255,
    ((value >> 8) & 255) / 255,
    (value & 255) / 255,
  ];
}

/** The station's look. */
export const LOOK: Look = {
  palette: {
    panel: hex(0xf4f6f8),
    floor: hex(0x6c7074),
    ceiling: hex(0xe4e8ec),
    metal: hex(0x8c9094),
    door: hex(0x28d8ff),
    portal: hex(0xff8a1c),
    portalAlt: hex(0x28d8ff),
    machine: hex(0xd8dce0),
    screen: hex(0x06121a),
    screenText: hex(0x9ae8ff),
    lamp: hex(0xf0f8ff),
    fog: hex(0x0c1014),
  },
  propBody: { body: hex(0xb4b4ae), panel: hex(0xc3c3bd) },
  lightScale: 1,
  falloff: 0.05,
  minLight: 0.1,
  bands: 32,
  grime: 0.04,
  contactShadow: 0.85,
  edge: {
    colour: hex(0x9ae8ff),
    strength: 2.2,
    everywhere: true,
    width: 1.4,
    fade: { from: 15, to: 25 },
  },
  bloom: { threshold: 0.9, strength: 0.8 },
  // Safety yellow, signal red, leaf green, violet, teal.
  accents: [
    hex(0xf0d830),
    hex(0xd8382e),
    hex(0x5a9a40),
    hex(0x845cc8),
    hex(0x1f9e8c),
  ],
  propAccents: true,
};

/**
 * The colour a room's accent takes in `look` (2.7 C8): the look's accent at
 * the room's index, `look.accents[room.finish.accent]`. The renderer
 * uploads it as `uAccent` on every draw, so a restored context, which
 * hands the same room back, keeps it. Black for an index past the set,
 * which no finish holds.
 */
export function accentFor(room: { finish: Finish }, look: Look): Rgb {
  return look.accents[room.finish.accent] ?? [0, 0, 0];
}

/**
 * The look the set dressing, the machines, the terminals and the furniture
 * are built in: `look` with its `machine` and `panel` swapped for
 * `propBody`, or `look` itself when it has none. The room's shell and its
 * ways, the heroes and the curios keep the plain look.
 */
export function propLook(look: Look): Look {
  if (look.propBody === undefined) return look;
  return {
    ...look,
    palette: {
      ...look.palette,
      machine: look.propBody.body,
      panel: look.propBody.panel,
    },
  };
}

/**
 * How many accents the look holds, and so how many a prop may pick from:
 * the scene shader's `uAccents` array is this long.
 */
export const ACCENT_COUNT = 5;

/**
 * The accent a prop picks from its seed, 0 to `ACCENT_COUNT - 1`: the same
 * seed always picks the same accent, whatever the look.
 */
export function propAccentPick(seed: number): number {
  return (seed >>> 0) % ACCENT_COUNT;
}

/**
 * A look adjusted for a room's condition: construction adds a little grime,
 * a retired room is darker and grimier, a derelict one much more so. The
 * light specials that make a retired room flicker come from the generator;
 * this only changes the surfaces.
 */
export function applyCondition(look: Look, condition: Condition): Look {
  const adjust: Record<Condition, { grime: number; light: number }> = {
    clean: { grime: 0, light: 1 },
    construction: { grime: 0.15, light: 0.95 },
    dim: { grime: 0.35, light: 0.8 },
    derelict: { grime: 0.7, light: 0.6 },
  };
  const a = adjust[condition];
  if (condition === "clean") return look;
  return {
    ...look,
    grime: Math.min(1, look.grime + a.grime),
    lightScale: look.lightScale * a.light,
  };
}

/** HSL to RGB; hue in degrees, saturation and lightness in [0, 1]. */
export function hueToRgb(
  hue: number,
  saturation: number,
  lightness: number,
): Rgb {
  const c = (1 - Math.abs(2 * lightness - 1)) * saturation;
  const h = (((hue % 360) + 360) % 360) / 60;
  const x = c * (1 - Math.abs((h % 2) - 1));
  const m = lightness - c / 2;
  const [r, g, b] =
    h < 1
      ? [c, x, 0]
      : h < 2
        ? [x, c, 0]
        : h < 3
          ? [0, c, x]
          : h < 4
            ? [0, x, c]
            : h < 5
              ? [x, 0, c]
              : [c, 0, x];
  return [r + m, g + m, b + m];
}
