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
    const all = hostSurfaces(room);
    const hosts = new Set(all.map((s) => s.host));
    const count = (o: object) => all.filter((s) => s.anchorOf === o).length;
    const machines: Partial<Record<string, readonly unknown[]>> =
      FIXTURE_SURFACES.machine;
    const decorTable: Partial<Record<string, readonly unknown[]>> =
      DECOR_SURFACES;
    const propTable: Partial<Record<string, readonly (readonly unknown[])[]>> =
      PROP_SURFACES;
    for (const f of room.fixtures) {
      const want =
        f.kind === "terminal"
          ? FIXTURE_SURFACES.terminal.length
          : f.kind === "machine"
            ? (machines[f.machine]?.length ?? 0)
            : 0;
      expect(count(f), f.kind).toBe(want);
    }
    for (const d of room.decor)
      expect(count(d), d.kind).toBe(decorTable[d.kind]?.length ?? 0);
    for (const p of room.props)
      expect(count(p), `${p.kind} ${String(p.variant)}`).toBe(
        p.anchor === "floor"
          ? (propTable[p.kind]?.[p.variant]?.length ?? 0)
          : 0,
      );
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
      const lab = { ...wb, machine: "lab-bench" as const };
      for (const fx of [term, wb, lab]) {
        const room = { ...base, fixtures: [fx] };
        const box = footprintOf(fx);
        if (box === null) throw new Error("footprint");
        for (const s of hostSurfaces(room))
          expect(inside(s.box, box), `${s.host} ${side}`).toBe(true);
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
    // Every hero at every turn, not only the one the hall stands it at.
    const hall = heroHallRoom();
    for (const h0 of hall.heroes)
      for (let t = 0; t < 4; t++) {
        const h = { ...h0, turn: t };
        const room = {
          ...hall,
          fixtures: [],
          decor: [],
          props: [],
          heroes: [h],
        };
        const got = hostSurfaces(room);
        const e = HERO_CATALOGUE[h.kind];
        expect(got.length, `${h.kind} ${String(t)}`).toBe(
          e.surfaces.length + e.under.length,
        );
        for (const s of got)
          expect(
            inside(s.box, heroFootprint(h)),
            `${h.kind} ${String(t)}`,
          ).toBe(true);
      }
  });

  it("gives open tops OPEN_CLEAR and every shelf level less", () => {
    const all = hostSurfaces(galleryRoom());
    for (const s of all) {
      expect(s.clear, s.host).toBeGreaterThan(0);
      expect(s.clear, s.host).toBeLessThanOrEqual(OPEN_CLEAR);
      // A desk, bench or table top is always open.
      if (s.cls === "desk" || s.cls === "bench" || s.cls === "table")
        expect(s.clear, s.host).toBe(OPEN_CLEAR);
      if (s.cls === "under") expect(s.clear, s.host).toBeLessThan(OPEN_CLEAR);
    }
    // A shelf host's highest level is its open top; every level below is
    // closed by the one above it.
    const shelves = new Set(
      all.filter((s) => s.cls === "shelf").map((s) => s.anchorOf),
    );
    expect(shelves.size).toBeGreaterThan(0);
    let closed = 0;
    for (const host of shelves) {
      const mine = all
        .filter((s) => s.anchorOf === host && s.cls === "shelf")
        .sort((a, b) => a.h - b.h);
      for (const [i, s] of mine.entries()) {
        if (i === mine.length - 1) expect(s.clear, s.host).toBe(OPEN_CLEAR);
        else {
          expect(s.clear, s.host).toBeLessThan(OPEN_CLEAR);
          closed++;
        }
      }
    }
    expect(closed).toBeGreaterThan(0);
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
    expect((b.z0 + b.z1) / 2).toBeCloseTo(5 * CELL, 9);
  });
});
