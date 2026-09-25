import { describe, expect, it } from "vitest";

import { CANNED_WORKSHOP, galleryRoom } from "../world/canned";
import { generateRoom } from "../world/generate";
import type { Hero } from "../world/types";
import { CELL } from "../world/units";
import { bankSlot } from "./blink";
import {
  INSTANCE_FLOATS,
  curioInstances,
  heroInstances,
  heroKey,
  instanceGroups,
  propInstances,
  propKey,
} from "./instances";
import { turnMat2Columns } from "./kit";
import { HERO_BANK } from "./models/heroes/common";
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

describe("instanceGroups", () => {
  // Review Focus 3's key-space pin: a hero key never collides with a prop
  // key (`props.has(g.key)` below), and grouping stays deterministic once
  // heroes are in the room (`instanceGroups(room)` called twice). It does
  // not exercise the rebuild Review Focus 3 also names, a look switch or a
  // restored context: no test here runs the renderer that rebuilds on.
  it("gives hero groups their own key space and their bank slot, the same arrays for the same room (Review Focus 3)", () => {
    const room = {
      ...galleryRoom(),
      heroes: [
        { kind: "turret", variant: 0, x: 4.5, y: 14, turn: 1, seed: 3 },
        { kind: "black-slab", variant: 0, x: 8.5, y: 13.925, turn: 2, seed: 4 },
      ] satisfies Hero[],
    };
    const groups = instanceGroups(room);
    const heroes = groups.filter((g) => g.family === "hero");
    expect(heroes.map((g) => g.key)).toEqual([
      "hero:black-slab:0",
      "hero:turret:0",
    ]);
    const props = new Set(
      groups.filter((g) => g.family === "prop").map((g) => g.key),
    );
    for (const g of heroes) expect(props.has(g.key)).toBe(false);
    const turret = heroes.find((g) => g.kind === "turret");
    expect(Array.from(turret?.data ?? [])).toEqual([
      4.5 * CELL,
      0,
      14 * CELL,
      1,
      bankSlot(HERO_BANK.turret),
    ]);
    const slab = heroes.find((g) => g.kind === "black-slab");
    expect(slab?.data[4]).toBe(0);
    expect(instanceGroups(room)).toEqual(groups);
    expect(
      propInstances(room).every(
        (g) =>
          g.family === "prop" &&
          g.data.every((_, i) => i % 5 !== 4 || g.data[i] === 0),
      ),
    ).toBe(true);
  });

  it("puts the props first, then the heroes, and a room without heroes gives only props", () => {
    const room = {
      ...galleryRoom(),
      heroes: [
        { kind: "core-wall", variant: 0, x: 6, y: 4, turn: 0, seed: 1 },
      ] satisfies Hero[],
    };
    const groups = instanceGroups(room);
    const families = groups.map((g) => g.family);
    expect(families.at(-1)).toBe("hero");
    expect(families.indexOf("hero")).toBe(propInstances(room).length);
    expect(heroInstances(room).map((g) => g.key)).toEqual([
      heroKey("core-wall", 0),
    ]);
    expect(heroInstances(room)[0]?.data[4]).toBe(bankSlot("twinkle"));
    const bare = { ...room, heroes: [] };
    expect(instanceGroups(bare)).toEqual(propInstances(bare));
  });

  it("writes one record per hero, grouped by kind and variant", () => {
    const room = {
      ...galleryRoom(),
      heroes: [
        { kind: "arcade-cabinet", variant: 2, x: 3, y: 5, turn: 0, seed: 1 },
        { kind: "arcade-cabinet", variant: 0, x: 7, y: 5, turn: 0, seed: 2 },
        { kind: "arcade-cabinet", variant: 2, x: 9, y: 5, turn: 3, seed: 3 },
      ] satisfies Hero[],
    };
    const groups = heroInstances(room);
    expect(groups.map((g) => [g.key, g.count])).toEqual([
      ["hero:arcade-cabinet:0", 1],
      ["hero:arcade-cabinet:2", 2],
    ]);
    expect(Array.from(groups[1]?.data ?? [])).toEqual([
      3 * CELL,
      0,
      5 * CELL,
      0,
      bankSlot("swap"),
      9 * CELL,
      0,
      5 * CELL,
      3,
      bankSlot("swap"),
    ]);
  });
});

describe("curioInstances", () => {
  // Review Focus 3's key-space pin for the third family: a curio key
  // never collides with a prop or hero key (the room holds all three
  // families), and each curio keeps its height and its kind's bank slot.
  it("gives curio groups their own key space, their height and their bank slot", () => {
    const room = {
      ...galleryRoom(),
      heroes: [
        { kind: "turret", variant: 0, x: 4.5, y: 14, turn: 1, seed: 3 },
      ] satisfies Hero[],
      curios: [
        {
          kind: "wing-meter" as const,
          variant: 0,
          x: 4.5,
          y: 3.25,
          h: 0.78,
          turn: 1,
          seed: 1,
        },
        {
          kind: "star-ball" as const,
          variant: 0,
          x: 6,
          y: 2,
          h: 1.08,
          turn: 0,
          seed: 2,
        },
      ],
    };
    const groups = curioInstances(room);
    expect(groups.map((g) => g.key)).toEqual([
      "curio:star-ball:0",
      "curio:wing-meter:0",
    ]);
    const meter = groups.find((g) => g.kind === "wing-meter");
    expect(Array.from(meter?.data ?? [])).toEqual([
      Math.fround(4.5 * CELL),
      Math.fround(0.78),
      Math.fround(3.25 * CELL),
      1,
      bankSlot("chase"),
    ]);
    const all = instanceGroups(room);
    expect(new Set(all.map((g) => g.family))).toEqual(
      new Set(["prop", "hero", "curio"]),
    );
    const keys = all.map((g) => g.key);
    expect(new Set(keys).size).toBe(keys.length);
    expect(instanceGroups(room).at(-1)?.family).toBe("curio");
    expect(curioInstances(room)).toEqual(curioInstances(room));
  });
});

describe("SCENE_VS", () => {
  it("carries the turn table built from turnMat2Columns", () => {
    // The whole array in turn order, so a table with two turns swapped
    // fails as well as one with a wrong matrix.
    const literals = [0, 1, 2, 3].map(
      (t) =>
        `mat2(${turnMat2Columns(t)
          .map((n) => n.toFixed(1))
          .join(", ")})`,
    );
    expect(SCENE_VS).toContain(`mat2[4](${literals.join(", ")})`);
    expect(SCENE_VS).toContain("layout(location = 6) in vec3 aInstanceOffset;");
    expect(SCENE_VS).toContain("layout(location = 7) in vec2 aInstanceTurn;");
  });
});
