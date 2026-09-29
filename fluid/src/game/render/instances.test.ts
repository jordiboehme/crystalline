import { describe, expect, it } from "vitest";

import {
  CANNED_BRIDGE,
  CANNED_HUB,
  CANNED_WORKSHOP,
  galleryRoom,
} from "../world/canned";
import { consoleRoom } from "../world/consoleRoom";
import { generateRoom } from "../world/generate";
import type { Hero, Prop } from "../world/types";
import { CELL } from "../world/units";
import { bankSlot } from "./blink";
import {
  INSTANCE_FLOATS,
  curioInstances,
  heroInstances,
  heroKey,
  instanceGroups,
  interiorInstances,
  interiorKey,
  propInstances,
  propKey,
} from "./instances";
import { turnMat2Columns } from "./kit";
import { HERO_BANK } from "./models/heroes/common";
import { INTERIOR_BANK } from "./models/interior/common";
import { PROP_BANK } from "./models/props/common";
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

  it("writes each prop's anchor in metres, its turn with its accent pick and its kind's bank slot", () => {
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
        p.turn + 4 * (1 + ((p.seed >>> 0) % 5)),
        bankSlot(PROP_BANK[p.kind]),
      ];
      for (let k = 0; k < INSTANCE_FLOATS; k++) {
        expect(floats[k]).toBeCloseTo(expected[k] ?? NaN, 5);
      }
      // The shader's turn is the float's low two bits.
      expect(Math.round(floats[3] ?? NaN) & 3).toBe(p.turn);
    }
  });

  it("picks each prop's own accent from its seed alone, the same every time", () => {
    // Mutation caught: a pick from the prop's position, kind or place in
    // the list (two props with one seed would differ, or the pick would
    // move with a neighbour), or a random pick (two builds would differ).
    const at = { anchor: "floor", y: 3.5, turn: 1, variant: 0 } as const;
    const picks = (props: Prop[]) =>
      propInstances({ ...workshop, props }).flatMap((g) =>
        Array.from({ length: g.count }, (_, i) =>
          Math.floor(Math.round(g.data[i * INSTANCE_FLOATS + 3] ?? NaN) / 4),
        ),
      );
    const one = (seed: number, x: number, kind: Prop["kind"]): Prop => ({
      ...at,
      kind,
      x,
      seed,
    });
    // One seed, two kinds, two places: one pick.
    const same = picks([one(77, 2.5, "crate"), one(77, 6.5, "barrel")]);
    expect(new Set(same).size).toBe(1);
    // The pick of seed 77 is 77 % 5 = 2, carried as 1 + 2.
    expect(same[0]).toBe(3);
    // Seeds 0 to 4 give all five picks, 1 to 5.
    const five = picks([0, 1, 2, 3, 4].map((s) => one(s, 2.5, "crate")));
    expect(new Set(five)).toEqual(new Set([1, 2, 3, 4, 5]));
    // A stray turn (5, one past a whole turn) wraps and keeps its pick.
    const stray = propInstances({
      ...workshop,
      props: [{ ...one(77, 2.5, "crate"), turn: 5 }],
    })[0]?.data[3];
    expect(stray).toBe(1 + 4 * 3);
    // The same room gives the same floats.
    expect(propInstances(workshop)).toEqual(propInstances(workshop));
  });

  it("gives a blinking prop its kind's bank slot and a crate the steady slot (2.6d C15)", () => {
    // Mutation caught: every prop written at slot 0, so the ooze never
    // breathes, or the bank read for the wrong kind.
    const at = { anchor: "floor", x: 3.5, y: 3.5, turn: 0, seed: 1 } as const;
    const room = {
      ...workshop,
      props: [
        { ...at, kind: "ooze-canisters", variant: 0 },
        { ...at, kind: "crate", variant: 0, x: 5.5 },
      ] satisfies Prop[],
    };
    const groups = propInstances(room);
    const slotOf = (kind: string) => {
      const g = groups.find((x) => x.kind === kind);
      if (g === undefined) throw new Error(`no ${kind} group`);
      return g.data[4];
    };
    expect(bankSlot("breathe")).not.toBe(0);
    expect(slotOf("ooze-canisters")).toBe(bankSlot("breathe"));
    expect(slotOf("crate")).toBe(0);
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
  // not exercise the rebuild Review Focus 3 also names, a new look (only a
  // dev page picks one; the look is fixed in play) or a restored context:
  // no test here runs the renderer that rebuilds on.
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
          g.data.every(
            (_, i) => i % 5 !== 4 || g.data[i] === bankSlot(PROP_BANK[g.kind]),
          ),
      ),
    ).toBe(true);
  });

  it("puts the props first, then the heroes, and a room without heroes gives only props", () => {
    const room = {
      ...galleryRoom(),
      // Curios are their own family (see "gives curio groups their own key
      // space" below); stripped here so this test's own claim (props, then
      // heroes) isn't muddied by where a curio group falls.
      curios: [],
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
  });
});

describe("interiorInstances (2.6e C2)", () => {
  it("groups the console room's fittings in their own key space, after every other family", () => {
    // Mutation caught: the interior family left out of `instanceGroups`,
    // a key without its prefix (it could meet a prop's), or the console's
    // slot not its twinkling bank.
    const room = consoleRoom();
    const pieces = room.interior ?? [];
    expect(pieces.length).toBeGreaterThan(0);
    const groups = interiorInstances(room);
    expect(groups.reduce((n, g) => n + g.count, 0)).toBe(pieces.length);
    for (const g of groups) {
      expect(g.family).toBe("interior");
      expect(g.key).toBe(`interior:${g.kind}:${String(g.variant)}`);
      expect(g.key).toBe(interiorKey(g.kind, g.variant));
      for (let i = 0; i < g.count; i++) {
        expect(g.data[i * INSTANCE_FLOATS + 1]).toBe(0);
        expect(g.data[i * INSTANCE_FLOATS + 4]).toBe(
          bankSlot(INTERIOR_BANK[g.kind]),
        );
      }
    }
    const desk = groups.find((g) => g.kind === "console");
    expect(Array.from(desk?.data ?? [])).toEqual([
      3 * CELL,
      0,
      3 * CELL,
      0,
      bankSlot("twinkle"),
    ]);
    const all = instanceGroups(room);
    expect(all.slice(-groups.length)).toEqual(groups);
    expect(all.filter((g) => g.family === "interior")).toEqual(groups);
  });

  it("gives every generated canned room no interior group", () => {
    // Mutation caught: a generated room given fittings (a golden would
    // move) or an interior group made from nothing.
    for (const place of [CANNED_BRIDGE, CANNED_WORKSHOP, CANNED_HUB]) {
      const room = generateRoom(place);
      expect(room.interior).toBeUndefined();
      expect(interiorInstances(room)).toEqual([]);
      expect(instanceGroups(room).some((g) => g.family === "interior")).toBe(
        false,
      );
    }
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
