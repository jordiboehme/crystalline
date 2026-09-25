import { describe, expect, it } from "vitest";

import {
  FLAG,
  FLOATS_PER_VERTEX,
  createBuilder,
  type MeshData,
  type Surface,
} from "./geometry";
import { createKit, frameForSlot } from "./kit";
import {
  floatingGlow,
  recordingKitAt,
  worstWinding,
  type Part,
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
