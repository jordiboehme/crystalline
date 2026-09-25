import { describe, expect, it } from "vitest";

import { BLINK_GROUPS } from "./blink";
import {
  FLAG,
  FLOATS_PER_VERTEX,
  blinkFlag,
  createBuilder,
  type MeshData,
  type Surface,
  type V3,
} from "./geometry";
import { createKit, frameForSlot } from "./kit";
import {
  GLOWING,
  floatingGlow,
  looseParts,
  recordingKitAt,
  shape,
  touching,
  worstWinding,
  type Part,
  type Shape,
} from "./modelChecks";

const LIT: Surface = { layer: 1, tint: [0.5, 0.5, 0.5], flag: FLAG.lit };
const GLOW: Surface = { layer: 1, tint: [1, 1, 1], flag: FLAG.emissive };
const WALL = frameForSlot({ x: 3, y: 4, side: "n" });

/**
 * A lit box against the wall (`a` -0.5 to 0.5, `d` 0 to 0.5, `h` 0 to 0.5)
 * and one glowing panel facing into the room, as `place` puts it.
 */
function scene(place: (k: ReturnType<typeof createKit>) => void): Part[] {
  const parts: Part[] = [];
  const kitAt = recordingKitAt(createBuilder(), parts);
  const k = kitAt(WALL);
  k.box(-0.5, 0.5, 0, 0.5, 0, 0.5, LIT);
  place(k);
  return parts;
}

describe("floatingGlow", () => {
  it("names a glowing part that floats 0.2 m in front of its box", () => {
    const parts = scene((k) => k.panel(-0.2, 0.2, 0.7, 0.1, 0.4, GLOW));
    expect(floatingGlow(parts, null)).toEqual(["1:panel"]);
    expect(floatingGlow(parts, WALL)).toEqual(["1:panel"]);
  });

  it("passes a glowing part that touches its box", () => {
    const parts = scene((k) => k.panel(-0.2, 0.2, 0.51, 0.1, 0.4, GLOW));
    expect(floatingGlow(parts, null)).toEqual([]);
    expect(floatingGlow(parts, WALL)).toEqual([]);
  });

  it("passes a glowing part on the wall plane only when given the wall", () => {
    const parts = scene((k) => k.panel(1.0, 1.4, 0.01, 1.0, 1.4, GLOW));
    expect(floatingGlow(parts, WALL)).toEqual([]);
    expect(floatingGlow(parts, null)).toEqual(["1:panel"]);
  });
});

describe("GLOWING", () => {
  it("holds the signal flag and every blink group's flag, and not the ceiling lamp", () => {
    expect(GLOWING).toContain(FLAG.signal);
    for (let g = 0; g < BLINK_GROUPS; g++)
      expect(GLOWING).toContain(blinkFlag(g));
    expect(GLOWING).not.toContain(FLAG.lamp);
    expect(GLOWING).not.toContain(FLAG.lit);
  });

  it("names a floating signal light and a floating blinking one", () => {
    const signal: Surface = { ...GLOW, flag: FLAG.signal };
    const blink: Surface = { ...GLOW, flag: blinkFlag(BLINK_GROUPS - 1) };
    expect(
      floatingGlow(
        scene((k) => k.panel(-0.2, 0.2, 0.7, 0.1, 0.4, signal)),
        null,
      ),
    ).toEqual(["1:panel"]);
    expect(
      floatingGlow(
        scene((k) => k.panel(-0.2, 0.2, 0.7, 0.1, 0.4, blink)),
        null,
      ),
    ).toEqual(["1:panel"]);
  });
});

describe("floatingGlow on a frame", () => {
  const FRAME: Surface = { layer: 1, tint: [1, 1, 1], flag: FLAG.frame };
  const SIGNAL: Surface = { layer: 1, tint: [1, 1, 1], flag: FLAG.signal };

  /** A frame post standing off the wall, 1 m clear of the lit box, and `place`. */
  function framed(place: (k: ReturnType<typeof createKit>) => void): Part[] {
    return scene((k) => {
      k.box(1.5, 1.7, 0.2, 0.4, 0, 1, FRAME);
      place(k);
    });
  }

  it("lets a signal light and a blinking one sit on a frame, which alone is named", () => {
    // The frame post stands off the wall with no lit host, so the frame
    // itself floats and is named ("1:box"); the light on it is not.
    const signal = framed((k) => k.panel(1.55, 1.65, 0.41, 0.5, 0.6, SIGNAL));
    expect(floatingGlow(signal, WALL)).toEqual(["1:box"]);
    const blink: Surface = { ...SIGNAL, flag: blinkFlag(3) };
    const blinking = framed((k) => k.panel(1.55, 1.65, 0.41, 0.5, 0.6, blink));
    expect(floatingGlow(blinking, WALL)).toEqual(["1:box"]);
  });

  it("does not let a screen sit on a frame: only signal and blink lights may", () => {
    const parts = framed((k) => k.panel(1.55, 1.65, 0.41, 0.5, 0.6, GLOW));
    expect(floatingGlow(parts, WALL)).toEqual(["1:box", "2:panel"]);
  });

  it("still names a frame with no lit host, and a light that floats off the frame", () => {
    const parts = framed((k) => k.panel(1.55, 1.65, 0.6, 0.5, 0.6, SIGNAL));
    expect(floatingGlow(parts, WALL)).toEqual(["1:box", "2:panel"]);
  });
});

describe("worstWinding", () => {
  /** A bevelled box, a mesh whose winding is right everywhere. */
  function goodMesh(): MeshData {
    const b = createBuilder();
    createKit(b, WALL).bevelBox(-0.5, 0.5, 0, 0.5, 0, 0.5, 0.05, LIT);
    return b.build();
  }

  it("is 1 for a mesh wound with its normals", () => {
    expect(worstWinding(goodMesh())).toBeGreaterThan(0.999);
  });

  it("drops below 0 when one triangle's vertex order is swapped", () => {
    const m = goodMesh();
    const v = m.vertices;
    const second = v.slice(FLOATS_PER_VERTEX, 2 * FLOATS_PER_VERTEX);
    v.copyWithin(
      FLOATS_PER_VERTEX,
      2 * FLOATS_PER_VERTEX,
      3 * FLOATS_PER_VERTEX,
    );
    v.set(second, 2 * FLOATS_PER_VERTEX);
    expect(worstWinding(m)).toBeLessThan(0);
  });

  it("drops below 0 when one triangle's normals are negated", () => {
    const m = goodMesh();
    for (let i = 0; i < 3; i++) {
      for (let k = 3; k < 6; k++) {
        const o = i * FLOATS_PER_VERTEX + k;
        m.vertices[o] = -(m.vertices[o] ?? 0);
      }
    }
    expect(worstWinding(m)).toBeLessThan(0);
  });
});

describe("touching", () => {
  /** A shape's bounds from its two extreme corners: `stacked`, `sleeve` and
   * `overlapsVolume` read only the bounds, and the `near` fallback never
   * runs once one of them already says yes, so two points are enough. */
  const box = (lo: V3, hi: V3): Shape => shape([lo, hi]);

  it("does not touch a shape floating 5 cm above it", () => {
    const held = box([0, 0, 0], [0.2, 0, 0.2]);
    const floater = box([0, 0.05, 0], [0.2, 0.05, 0.2]);
    expect(touching(held, floater)).toBe(false);
    expect(touching(floater, held)).toBe(false);
  });

  it("does not touch a shape 1.5 cm clear, the shelf gap TOUCH keeps out", () => {
    const lower = box([0, 0, 0], [0.2, 0.3, 0.2]);
    const upper = box([0, 0.315, 0], [0.2, 0.4, 0.2]);
    expect(touching(lower, upper)).toBe(false);
    expect(touching(upper, lower)).toBe(false);
  });

  it("touches a shape it runs through by more than 1 cm on every axis", () => {
    // The laser desk's arm through the emitter housing.
    const housing = box([0, 0, 0], [0.3, 0.3, 0.3]);
    const arm = box([0.1, 0.1, 0.1], [0.4, 0.4, 0.4]);
    expect(touching(housing, arm)).toBe(true);
  });

  it("touches a shape it sits flush on even where the footprint steps out", () => {
    // The dome planter's collar (r 0.7) flaring past its drum (r 0.68),
    // their heights meeting exactly at 0.36: no shared height for `sleeve`
    // or `overlapsVolume`, and the 2 cm step is wider than `TOUCH`.
    const drum = box([-0.68, 0.1, -0.68], [0.68, 0.36, 0.68]);
    const collar = box([-0.7, 0.36, -0.7], [0.7, 0.42, 0.7]);
    expect(touching(drum, collar)).toBe(true);
    expect(touching(collar, drum)).toBe(true);
  });

  it("does not call two shapes stacked when their footprints barely meet", () => {
    // Flush in height, but their plan boxes clip corners by under 1 cm:
    // not a real rest, just two boxes that happen to touch a shared plane.
    const a = box([0, 0, 0], [0.2, 0.3, 0.2]);
    const b = box([0.195, 0.3, 0.195], [0.4, 0.5, 0.4]);
    expect(touching(a, b)).toBe(false);
  });
});

describe("looseParts", () => {
  /** A flat triangle `Part` at a given height, with no flag or layer. */
  const partAt = (method: string, y: number): Part => ({
    builder: {},
    method,
    layer: -1,
    flag: -1,
    points: [
      [0, y, 0],
      [0.1, y, 0],
      [0, y, 0.1],
    ],
  });

  it("names a synthetic floater with no path back to the floor", () => {
    const held = partAt("floor", 0);
    const floater: Part = {
      ...partAt("floater", 1),
      points: [
        [5, 1, 5],
        [5.1, 1, 5],
        [5, 1, 5.1],
      ],
    };
    expect(looseParts([held, floater], null)).toEqual(["1:floater"]);
  });

  it("holds a part two touches away from the floor, through the fixed point", () => {
    const held = partAt("floor", 0);
    // Within TOUCH (1.2 cm) of `held`: joins on the first pass.
    const mid = partAt("mid", 0.005);
    // Within TOUCH of `mid` but 1.5 cm clear of `held` directly: joins
    // only once `mid` is already held, on the loop's second pass.
    const far = partAt("far", 0.015);
    expect(looseParts([held, mid, far], null)).toEqual([]);
    expect(looseParts([held, far], null)).toEqual(["1:far"]);
  });

  it("drops a part with no points rather than counting it loose", () => {
    const held = partAt("floor", 0);
    const empty: Part = {
      builder: {},
      method: "empty",
      layer: -1,
      flag: -1,
      points: [],
    };
    expect(looseParts([held, empty], null)).toEqual([]);
  });
});
