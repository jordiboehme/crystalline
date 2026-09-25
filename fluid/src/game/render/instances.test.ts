import { describe, expect, it } from "vitest";

import { CANNED_WORKSHOP } from "../world/canned";
import { generateRoom } from "../world/generate";
import { CELL } from "../world/units";
import { INSTANCE_FLOATS, propInstances, propKey } from "./instances";
import { turnMat2Columns } from "./kit";
import { SCENE_VS } from "./shaders";

const workshop = generateRoom(CANNED_WORKSHOP);

describe("propInstances", () => {
  it("has props to group on the workshop", () => {
    expect(workshop.props.length).toBeGreaterThan(0);
  });

  it("counts every prop exactly once", () => {
    const groups = propInstances(workshop);
    const total = groups.reduce((n, g) => n + g.count, 0);
    expect(total).toBe(workshop.props.length);
    for (const g of groups) {
      expect(g.data.length).toBe(g.count * INSTANCE_FLOATS);
    }
  });

  it("makes one group per kind and variant, sorted by key", () => {
    const groups = propInstances(workshop);
    const keys = groups.map((g) => g.key);
    const distinct = [
      ...new Set(workshop.props.map((p) => propKey(p.kind, p.variant))),
    ];
    expect(keys).toEqual([...distinct].sort());
    for (const g of groups) expect(g.key).toBe(propKey(g.kind, g.variant));
  });

  it("writes each prop's anchor in metres, its turn and a zero slot", () => {
    const groups = propInstances(workshop);
    const seen = new Map<string, number>();
    for (const p of workshop.props) {
      const key = propKey(p.kind, p.variant);
      const g = groups.find((x) => x.key === key);
      expect(g).toBeDefined();
      if (g === undefined) continue;
      const i = seen.get(key) ?? 0;
      seen.set(key, i + 1);
      const at = i * INSTANCE_FLOATS;
      const floats = Array.from(g.data.subarray(at, at + INSTANCE_FLOATS));
      const expected = [
        p.x * CELL,
        p.anchor === "ceiling" ? workshop.ceiling : 0,
        p.y * CELL,
        p.turn,
        0,
      ];
      for (let k = 0; k < INSTANCE_FLOATS; k++) {
        expect(floats[k]).toBeCloseTo(expected[k] ?? NaN, 5);
      }
    }
  });

  it("gives nothing for a room without props", () => {
    expect(propInstances({ ...workshop, props: [] })).toEqual([]);
  });

  it("gives the same arrays for the same room", () => {
    const a = propInstances(workshop);
    const b = propInstances(workshop);
    expect(b.map((g) => g.key)).toEqual(a.map((g) => g.key));
    a.forEach((g, i) => {
      expect(Array.from(b[i]?.data ?? [])).toEqual(Array.from(g.data));
    });
  });
});

describe("SCENE_VS", () => {
  it("carries the turn table built from turnMat2Columns", () => {
    for (const t of [0, 1, 2, 3]) {
      const literal = `mat2(${turnMat2Columns(t)
        .map((n) => n.toFixed(1))
        .join(", ")})`;
      expect(SCENE_VS).toContain(literal);
    }
    expect(SCENE_VS).toContain("layout(location = 6) in vec3 aInstanceOffset;");
    expect(SCENE_VS).toContain("layout(location = 7) in vec2 aInstanceTurn;");
  });
});
