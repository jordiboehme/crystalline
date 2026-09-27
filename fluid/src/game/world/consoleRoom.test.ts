import { describe, expect, it } from "vitest";

import {
  INTERIOR_CATALOGUE,
  atConsoleExit,
  consoleRoom,
  interiorFootprint,
} from "./consoleRoom";
import { arrivalSpawn } from "./interact";
import { edgeKey, edgeOf } from "./sites";
import type { InteriorKind } from "./types";

describe("the console room (2.6e C1, C3)", () => {
  it("is the same room every time, byte for byte", () => {
    // Mutation caught: anything seeded from a clock or a counter.
    expect(JSON.stringify(consoleRoom())).toBe(JSON.stringify(consoleRoom()));
  });

  it("is a 12 m square, 4 m high, with nothing a generated room carries", () => {
    // Mutation caught: the ceiling or the grid off its size, or a
    // generated room's contents carried in.
    const r = consoleRoom();
    expect([r.width, r.depth, r.ceiling]).toEqual([6, 6, 4]);
    expect(r.grid.every((row) => row === "......")).toBe(true);
    expect([
      r.fixtures,
      r.decor,
      r.scaffold,
      r.heroes,
      r.props,
      r.curios,
    ]).toEqual([[], [], [], [], [], []]);
    expect(r.title).toBe("");
  });

  it("covers every wall edge with one flush piece and stands the console at the centre", () => {
    // Mutation caught: a wall edge left bare, a piece on two edges, the
    // doors off centre.
    const r = consoleRoom();
    const pieces = r.interior ?? [];
    expect(pieces.length).toBeGreaterThan(0);
    const count = (k: InteriorKind) =>
      pieces.filter((p) => p.kind === k).length;
    expect([
      count("roundel-wall"),
      count("inner-doors"),
      count("scanner"),
      count("console"),
    ]).toEqual([21, 1, 1, 1]);
    const doors = pieces.find((p) => p.kind === "inner-doors");
    expect(doors).toMatchObject({ x: 3, y: 6, turn: 0 });
    expect(pieces.find((p) => p.kind === "console")).toMatchObject({
      x: 3,
      y: 3,
    });
    const edges = new Set(
      pieces
        .filter((p) => p.kind === "roundel-wall" || p.kind === "scanner")
        .map((p) => edgeKey(edgeOf(p))),
    );
    expect(edges.size).toBe(22);
  });

  it("stands the player 1.6 m in front of the inner doors, facing the console", () => {
    // Mutation caught: the spawn off the doors' centre line, off 1.6 m, or
    // inside the way out.
    const s = arrivalSpawn(consoleRoom(), null);
    expect(s.x).toBeCloseTo(6);
    expect(s.z).toBeCloseTo(10.4);
    expect(s.yaw).toBe(0);
    expect(atConsoleExit(s)).toBe(false);
  });

  it("marks the doorway, and only the doorway, as the way out (2.6e C11)", () => {
    // Mutation caught: the half width or the reach off, or the zone on the
    // wrong wall.
    expect(atConsoleExit({ x: 6, z: 12 - 0.35 })).toBe(true);
    expect(atConsoleExit({ x: 6.9, z: 11.6 })).toBe(true);
    expect(atConsoleExit({ x: 7.1, z: 11.6 })).toBe(false);
    expect(atConsoleExit({ x: 6, z: 11.3 })).toBe(false);
    expect(atConsoleExit({ x: 6, z: 0.35 })).toBe(false);
  });

  it("gives the roundel walls every one of their variants", () => {
    // Mutation caught: the variant rule taken modulo fewer variants.
    const vs = new Set(
      (consoleRoom().interior ?? [])
        .filter((p) => p.kind === "roundel-wall")
        .map((p) => p.variant),
    );
    expect(vs).toEqual(new Set([0, 1, 2]));
  });

  it("gives the console, and only the console, a footprint round its desk (2.6e C8)", () => {
    // Mutation caught: a flush wall piece given a box (it would stand the
    // player off the walls), or the console's box off its centre or size.
    const pieces = consoleRoom().interior ?? [];
    expect(pieces.length).toBeGreaterThan(0);
    for (const p of pieces) {
      const box = interiorFootprint(p);
      if (INTERIOR_CATALOGUE[p.kind].footing === "flush") {
        expect(box, p.kind).toBeNull();
      } else {
        expect(box).toEqual({ x0: 4.8, x1: 7.2, z0: 4.8, z1: 7.2 });
      }
    }
  });
});
