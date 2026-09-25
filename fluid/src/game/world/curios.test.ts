/**
 * The curios' world side (2.6b): the catalogue, the host surface tables,
 * the shared local-to-world transform and `hostSurfaces`. The pass's own
 * tests (Task 6) join this file later.
 */
import { describe, expect, it } from "vitest";

import { frameAt, frameForSlot } from "../render/kit";
import { galleryRoom, heroHallRoom } from "./canned";
import {
  CURIO_CATALOGUE,
  CURIO_KINDS,
  DECOR_SURFACES,
  FIXTURE_SURFACES,
  PROP_SURFACES,
  curioBox,
  hostSurfaces,
} from "./curios";
import {
  OPEN_CLEAR,
  decorFootprint,
  footprintOf,
  heroFootprint,
  propFootprint,
  turnedPoint,
} from "./footprints";
import { HERO_CATALOGUE, HERO_KINDS } from "./heroes";
import { PROP_KINDS } from "./props";
import { wallAnchor } from "./sites";
import type { Box, Side, WallSlot } from "./types";
import { CELL } from "./units";

const SIDES: readonly Side[] = ["n", "e", "s", "w"];
const inside = (inner: Box, outer: Box, eps = 1e-6) =>
  inner.x0 >= outer.x0 - eps &&
  inner.x1 <= outer.x1 + eps &&
  inner.z0 >= outer.z0 - eps &&
  inner.z1 <= outer.z1 + eps;

describe("curio catalogue", () => {
  it("names every kind once, apart from every prop and hero kind", () => {
    expect(new Set(CURIO_KINDS).size).toBe(CURIO_KINDS.length);
    expect(Object.keys(CURIO_CATALOGUE).sort()).toEqual(
      [...CURIO_KINDS].sort(),
    );
    for (const k of CURIO_KINDS) {
      expect((PROP_KINDS as readonly string[]).includes(k), k).toBe(false);
      expect((HERO_KINDS as readonly string[]).includes(k), k).toBe(false);
    }
  });

  it("gives every variant a size, every kind a slot, a class and a facing", () => {
    for (const k of CURIO_KINDS) {
      const e = CURIO_CATALOGUE[k];
      expect(e.sizes.length, k).toBe(e.variants);
      for (const s of e.sizes) {
        expect(s.width, k).toBeGreaterThan(0);
        expect(s.depth, k).toBeGreaterThan(0);
        expect(s.top, k).toBeGreaterThan(0);
        expect(Math.max(s.width, s.depth), k).toBeLessThanOrEqual(0.65);
      }
      expect(e.classes.length, k).toBeGreaterThan(0);
      expect(e.slot === "under", k).toBe(e.classes.includes("under"));
    }
  });

  it("keeps the laptop off the terminal ends (C8)", () => {
    const [laptop] = CURIO_CATALOGUE["beige-laptop"].sizes;
    const [end] = FIXTURE_SURFACES.terminal;
    if (laptop === undefined || end === undefined) throw new Error("tables");
    expect(end.a1 - end.a0).toBeLessThan(Math.min(laptop.width, laptop.depth));
  });
});

describe("turnedPoint (C4)", () => {
  it("matches frameAt at every turn", () => {
    for (let t = 0; t < 4; t++) {
      const f = frameAt([3 * CELL, 0, 5 * CELL], t);
      for (const [a, d] of [
        [0.3, -0.2],
        [-1.1, 0.4],
        [0, 0.9],
      ] as const) {
        const p = turnedPoint(3, 5, t, a, d);
        expect(p.x).toBeCloseTo(
          f.origin[0] + f.along[0] * a + f.inward[0] * d,
          9,
        );
        expect(p.z).toBeCloseTo(
          f.origin[2] + f.along[2] * a + f.inward[2] * d,
          9,
        );
      }
    }
  });

  it("matches frameForSlot on every wall side, from the slot's wall anchor", () => {
    for (const side of SIDES) {
      const slot: WallSlot = { x: 3, y: 4, side };
      const f = frameForSlot(slot);
      const w = wallAnchor(slot);
      for (const [a, d] of [
        [0.5, 0.3],
        [-0.66, 0.56],
        [0.2, 0.06],
      ] as const) {
        const p = turnedPoint(w.x, w.y, w.turn, a, d);
        expect(p.x, side).toBeCloseTo(
          f.origin[0] + f.along[0] * a + f.inward[0] * d,
          9,
        );
        expect(p.z, side).toBeCloseTo(
          f.origin[2] + f.along[2] * a + f.inward[2] * d,
          9,
        );
      }
    }
  });
});

describe("host surfaces", () => {
  it("finds every host kind of the gallery with its tables' counts", () => {
    const room = galleryRoom();
    const hosts = new Set(hostSurfaces(room).map((s) => s.host));
    for (const h of [
      "terminal",
      "machine:workbench",
      "machine:lab-bench",
      "decor:lab-island",
      "decor:round-table",
      "prop:storage-shelf",
      "prop:filing-cabinet",
    ])
      expect(hosts.has(h), h).toBe(true);
  });

  it("finds every hero top and under spot of the hero hall", () => {
    const room = heroHallRoom();
    const all = hostSurfaces(room);
    for (const h of room.heroes) {
      const e = HERO_CATALOGUE[h.kind];
      const mine = all.filter(
        (s) => s.host === `hero:${h.kind}` && s.anchorOf === h,
      );
      expect(mine.length, h.kind).toBe(e.surfaces.length + e.under.length);
    }
  });

  it("puts every surface box inside its host's footprint, at every side and turn (C5)", () => {
    // One host of each table at every side or turn, on its own.
    for (let t = 0; t < 4; t++) {
      const side = SIDES.find(
        (s) => wallAnchor({ x: 3, y: 4, side: s }).turn === t,
      );
      if (side === undefined) throw new Error("side");
      const slot: WallSlot = { x: 3, y: 4, side };
      const base = { ...galleryRoom(), heroes: [], props: [], decor: [] };
      const term = {
        kind: "terminal" as const,
        slot,
        heading: "x",
        lines: [],
        section: 0,
        seed: 1,
      };
      const wb = {
        kind: "machine" as const,
        slot,
        machine: "workbench" as const,
        tag: "t",
        hue: 0,
        seed: 1,
      };
      for (const fx of [term, wb]) {
        const room = { ...base, fixtures: [fx] };
        const box = footprintOf(fx);
        if (box === null) throw new Error("footprint");
        for (const s of hostSurfaces(room))
          expect(inside(s.box, box), `${fx.kind} ${side}`).toBe(true);
      }
      for (const kind of Object.keys(
        DECOR_SURFACES,
      ) as (keyof typeof DECOR_SURFACES)[]) {
        const d = { kind, x: 8.5, y: 6, turn: t, seed: 1 };
        const room = { ...base, fixtures: [], decor: [d] };
        const box = decorFootprint(d);
        if (box === null) throw new Error("decor footprint");
        for (const s of hostSurfaces(room))
          expect(inside(s.box, box), `${kind} ${t}`).toBe(true);
      }
      for (const kind of Object.keys(
        PROP_SURFACES,
      ) as (keyof typeof PROP_SURFACES)[]) {
        const variants = PROP_SURFACES[kind]?.length ?? 0;
        for (let v = 0; v < variants; v++) {
          const p = {
            kind,
            variant: v,
            anchor: "floor" as const,
            x: 8.5,
            y: 6.5,
            turn: t,
            seed: 1,
          };
          const room = { ...base, fixtures: [], props: [p] };
          const box = propFootprint(p);
          if (box === null) throw new Error("prop footprint");
          for (const s of hostSurfaces(room))
            expect(inside(s.box, box), `${kind} ${v} ${t}`).toBe(true);
        }
      }
    }
    for (const h of heroHallRoom().heroes) {
      const room = { ...heroHallRoom(), heroes: [h] };
      for (const s of hostSurfaces(room))
        expect(inside(s.box, heroFootprint(h)), h.kind).toBe(true);
    }
  });

  it("gives open tops OPEN_CLEAR and every shelf level less", () => {
    for (const s of hostSurfaces(galleryRoom())) {
      expect(s.clear).toBeGreaterThan(0);
      expect(s.clear).toBeLessThanOrEqual(OPEN_CLEAR);
    }
  });
});

describe("curioBox", () => {
  it("is the variant's size centred on the curio, swapped at an odd turn", () => {
    const [size] = CURIO_CATALOGUE["beige-laptop"].sizes;
    if (size === undefined) throw new Error("size");
    const c = {
      kind: "beige-laptop" as const,
      variant: 0,
      x: 4,
      y: 5,
      h: 0.9,
      turn: 1,
      seed: 0,
    };
    const b = curioBox(c);
    expect(b.x1 - b.x0).toBeCloseTo(size.depth, 9);
    expect(b.z1 - b.z0).toBeCloseTo(size.width, 9);
    expect((b.x0 + b.x1) / 2).toBeCloseTo(4 * CELL, 9);
  });
});
