/**
 * The three looks the demo offers, as parameter sets over the same room.
 *
 * A look changes nothing about what is in a room, only how it is drawn: the
 * palette, how bright the light is and how far it carries, how much grime,
 * how strong the neon edge lines are and where they appear, whether surfaces
 * are textured or flat, whether the picture is dithered down to the C64's
 * sixteen colours, and the five accents a room is picked out in (2.7 C7).
 * The renderer reads nothing else, so a look costs a few dozen numbers.
 *
 * - Day shift: beige and off-white panels in bright light, neon only on
 *   doors, portals and tag strips.
 * - Aperture grid: white panels with glowing seams everywhere, doors in cyan
 *   and portals in orange.
 * - Freescape 64: flat-shaded solid polygons, ordered dithering and the
 *   C64 palette, after Driller and the other Freescape games.
 *
 * The one the demo's player picks becomes the default; the others stay
 * behind a hidden key only if they cost almost nothing.
 */

import type { Condition, Finish } from "../world/types";

/**
 * A colour, each channel in [0, 1]. The one exception is a surface's
 * accent mark (`accentTint` in `geometry.ts`), a negative first channel the
 * vertex shader swaps for the room's accent.
 */
export type Rgb = readonly [number, number, number];

/** Which look. */
export type LookId = "day" | "aperture" | "freescape";

/** Every parameter the renderer reads from a look. */
export interface Look {
  id: LookId;
  name: string;
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
  /** Multiplies every light level. */
  lightScale: number;
  /** How quickly light falls off with distance (DOOM's diminishing light). */
  falloff: number;
  /** The darkest a lit surface gets, 0 to 1. */
  minLight: number;
  /** How many brightness bands the falloff is quantised to (DOOM: 32). */
  bands: number;
  /** 0 clean to 1 filthy. */
  grime: number;
  edge: {
    colour: Rgb;
    /** Added brightness of an edge line; above 1 feeds the bloom. */
    strength: number;
    /** Seams on every wall panel, not only on frames. */
    everywhere: boolean;
    /** Line width in pixels. */
    width: number;
  };
  /** 1 textured, 0 solid colours only. */
  textureMix: number;
  /** Face normals, no smooth light across a surface. */
  flat: boolean;
  /** Ordered dithering into `C64_PALETTE` in the final pass. */
  dither: boolean;
  bloom: { threshold: number; strength: number };
  /** The terminal screens' style. */
  terminal: "phosphor" | "petscii";
  /**
   * The look's accent set (2.7 C7): `ACCENT_COUNT` colours, index `i` in
   * the same colour family in every look (0 yellow, 1 red, 2 green, 3 blue,
   * cyan in the C64 palette, 4 teal), each at least 0.25 (RGB distance)
   * from the look's `door`, `portal` and `portalAlt`, so an accent never
   * reads as a way. A room picks one by index (`Finish.accent`) and
   * `accentFor` reads it.
   */
  accents: readonly Rgb[];
}

const C64_RAW: readonly (readonly [number, number, number])[] = [
  [0x00, 0x00, 0x00],
  [0xff, 0xff, 0xff],
  [0x68, 0x37, 0x2b],
  [0x70, 0xa4, 0xb2],
  [0x6f, 0x3d, 0x86],
  [0x58, 0x8d, 0x43],
  [0x35, 0x28, 0x79],
  [0xb8, 0xc7, 0x6f],
  [0x6f, 0x4f, 0x25],
  [0x43, 0x39, 0x00],
  [0x9a, 0x67, 0x59],
  [0x44, 0x44, 0x44],
  [0x6c, 0x6c, 0x6c],
  [0x9a, 0xd2, 0x84],
  [0x6c, 0x5e, 0xb5],
  [0x95, 0x95, 0x95],
];

/** The C64's sixteen colours (the Pepto palette), in VIC-II order. */
export const C64_PALETTE: readonly Rgb[] = C64_RAW.map(
  ([r, g, b]) => [r / 255, g / 255, b / 255] as const,
);

function c64(index: number): Rgb {
  return C64_PALETTE[index] ?? [0, 0, 0];
}

function hex(value: number): Rgb {
  return [
    ((value >> 16) & 255) / 255,
    ((value >> 8) & 255) / 255,
    (value & 255) / 255,
  ];
}

/** The three looks, keyed by `LookId`. */
export const LOOKS: Record<LookId, Look> = {
  day: {
    id: "day",
    name: "Day shift",
    palette: {
      panel: hex(0xe8e0cc),
      floor: hex(0x9a9486),
      ceiling: hex(0xf2eee4),
      metal: hex(0xb8b2a4),
      door: hex(0xd87a2c),
      portal: hex(0x3cc8e8),
      portalAlt: hex(0xe85cc8),
      machine: hex(0xc9c1ad),
      screen: hex(0x0c1a10),
      screenText: hex(0x7cff9a),
      lamp: hex(0xfff6e0),
      fog: hex(0x1a1814),
    },
    lightScale: 1.1,
    falloff: 0.045,
    minLight: 0.12,
    bands: 32,
    grime: 0.08,
    edge: {
      colour: hex(0xffb050),
      strength: 1.6,
      everywhere: false,
      width: 1.2,
    },
    textureMix: 1,
    flat: false,
    dither: false,
    bloom: { threshold: 1, strength: 0.55 },
    terminal: "phosphor",
    // Mustard, rust red, olive, navy, teal green.
    accents: [
      hex(0xd8c040),
      hex(0xa83d28),
      hex(0x72803a),
      hex(0x2e4276),
      hex(0x338070),
    ],
  },
  aperture: {
    id: "aperture",
    name: "Aperture grid",
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
    lightScale: 1,
    falloff: 0.05,
    minLight: 0.1,
    bands: 32,
    grime: 0.04,
    edge: {
      colour: hex(0x9ae8ff),
      strength: 2.2,
      everywhere: true,
      width: 1.4,
    },
    textureMix: 1,
    flat: false,
    dither: false,
    bloom: { threshold: 0.9, strength: 0.8 },
    terminal: "phosphor",
    // Safety yellow, signal red, leaf green, violet, teal.
    accents: [
      hex(0xf0d830),
      hex(0xd8382e),
      hex(0x5a9a40),
      hex(0x845cc8),
      hex(0x1f9e8c),
    ],
  },
  freescape: {
    id: "freescape",
    name: "Freescape 64",
    palette: {
      panel: c64(15),
      floor: c64(11),
      ceiling: c64(12),
      metal: c64(12),
      door: c64(8),
      portal: c64(14),
      portalAlt: c64(4),
      machine: c64(3),
      screen: c64(6),
      screenText: c64(14),
      lamp: c64(1),
      fog: c64(0),
    },
    lightScale: 1,
    falloff: 0.06,
    minLight: 0.15,
    bands: 8,
    grime: 0,
    edge: { colour: c64(1), strength: 0.6, everywhere: false, width: 1 },
    textureMix: 0,
    flat: true,
    dither: true,
    bloom: { threshold: 1.2, strength: 0.2 },
    terminal: "petscii",
    // Yellow, light red, green, cyan, light green. The blue family takes
    // cyan: the palette's blue (6) sits 0.247 from the cross-domain portal
    // (4) and its light blue (14) is the portal itself.
    accents: [c64(7), c64(10), c64(5), c64(3), c64(13)],
  },
};

/**
 * The colour a room's accent takes in `look` (2.7 C8): the look's accent at
 * the room's index, `look.accents[room.finish.accent]`. The renderer
 * uploads it as `uAccent` on every draw, so a look switch or a restored
 * context, which hand the same room back, give the new look's colour of
 * the same family. Black for an index past the set, which no finish holds.
 */
export function accentFor(room: { finish: Finish }, look: Look): Rgb {
  return look.accents[room.finish.accent] ?? [0, 0, 0];
}

/** The looks in key order. */
export const LOOK_ORDER: readonly LookId[] = ["day", "aperture", "freescape"];

const KEYS: Record<string, LookId> = {
  Digit1: "day",
  Digit2: "aperture",
  Digit4: "freescape",
};

/** The look a key selects, by `KeyboardEvent.code`. Key 3 is deliberately free. */
export function lookForKey(code: string): LookId | null {
  return KEYS[code] ?? null;
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
