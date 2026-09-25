/**
 * The curios' world side (2.6b): the catalogue, the host surface tables,
 * the shared local-to-world transform and `hostSurfaces`, then the pass
 * itself (`curioDraws`, `placeCurios`): its shares, its invariants over
 * the canned places in every archetype and condition, the rooms where
 * nothing fits, the heights, the reach, the rates and one exact pin of a
 * surfaced hero with its curios (C11).
 */
import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { roomWithForcedHero } from "../dev/demo";
import { frameAt, frameForSlot } from "../render/kit";
import {
  CANNED_BRIDGE,
  CANNED_HUB,
  CANNED_WORKSHOP,
  galleryRoom,
  heroHallRoom,
} from "./canned";
import {
  BALL_FLOOR,
  BALL_SHARE,
  CURIO_CATALOGUE,
  CURIO_CEILING_GAP,
  CURIO_KINDS,
  CURIO_ORDER,
  DECOR_SURFACES,
  FIXTURE_SURFACES,
  GEAR_SHARE,
  PROP_SURFACES,
  RETRO_SHARE,
  UNDER_SHARE,
  curioBox,
  curioDraws,
  curioFits,
  curioOn,
  curioSize,
  curiosClash,
  hostSurfaces,
  placeCurios,
  type CurioDraws,
  type HostSurface,
} from "./curios";
import { dressRoom } from "./dress";
import {
  OPEN_CLEAR,
  decorFootprint,
  footprintOf,
  heroFootprint,
  propFootprint,
  turnedPoint,
} from "./footprints";
import { generateRoom } from "./generate";
import {
  HERO_CATALOGUE,
  HERO_KINDS,
  HERO_POOLS,
  heroCap,
  placeHeroes,
  type HeroDraws,
} from "./heroes";
import { blockersFor } from "./move";
import { PROP_KINDS } from "./props";
import { DEGENERATE_PLACES } from "./reachChecks";
import { dressingSites, overlaps, wallAnchor, type CurioBase } from "./sites";
import type {
  Archetype,
  Box,
  Condition,
  Curio,
  PlaceInput,
  RoomSpec,
  Side,
  WallSlot,
} from "./types";
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

const ARCHETYPE_TYPES = {
  bridge: "manifest",
  council: "decision",
  engineering: "runbook",
  archive: "reference",
  lab: "guide",
} satisfies Record<Archetype, string>;

const STATUS = {
  clean: "stable",
  construction: "draft",
  dim: "deprecated",
  derelict: "archived",
} satisfies Record<Condition, string>;

interface Made {
  name: string;
  archetype: Archetype;
  room: RoomSpec;
}

/** The room of `place` in every archetype and every condition. */
function matrix(place: PlaceInput): Made[] {
  const out: Made[] = [];
  for (const [archetype, type] of Object.entries(ARCHETYPE_TYPES))
    for (const [condition, status] of Object.entries(STATUS))
      out.push({
        name: `${place.permalink} ${archetype} ${condition}`,
        archetype: archetype as Archetype,
        room: generateRoom({ ...place, type, status }),
      });
  return out;
}

const OFF = { take: false, roll: 0 };
const ALL_FORCED: CurioDraws = {
  retro: { take: true, roll: 0.5 },
  gear: { take: true, roll: 0.5 },
  ball: { take: true, roll: 0.5, floor: false },
  under: { take: true, roll: 0.5 },
};
const WORKSHOPS = matrix(CANNED_WORKSHOP);
const ROOMS = [...WORKSHOPS, ...matrix(CANNED_BRIDGE), ...matrix(CANNED_HUB)];
const base = (r: RoomSpec): CurioBase => {
  const { curios, ...rest } = r;
  void curios;
  return rest;
};
const reseed = (r: RoomSpec, ...key: (string | number)[]): CurioBase => ({
  ...base(r),
  seed: seedFor(...key),
});
/** A count's tolerance: 4 standard deviations of `n` draws at `p`. */
const within = (count: number, n: number, p: number) =>
  Math.abs(count - n * p) <= 4 * Math.sqrt(n * p * (1 - p));
const contains = (outer: Box, inner: Box, eps = 1e-6) =>
  inner.x0 >= outer.x0 - eps &&
  inner.x1 <= outer.x1 + eps &&
  inner.z0 >= outer.z0 - eps &&
  inner.z1 <= outer.z1 + eps;
/** The surfaces a curio may stand on: each whose box holds its box, at its height. */
const hostsOf = (surfaces: readonly HostSurface[], c: Curio) =>
  surfaces.filter((s) => s.h === c.h && contains(s.box, curioBox(c)));

/**
 * Every invariant of one pass's output: at most one curio per slot, each
 * standing on a surface it fits (a floor ball apart), no two clashing, the
 * list sorted by `CURIO_ORDER`.
 */
function expectInvariants(
  room: CurioBase,
  curios: readonly Curio[],
  name: string,
) {
  const slots = curios.map((c) => CURIO_CATALOGUE[c.kind].slot);
  expect(new Set(slots).size, name).toBe(slots.length);
  const surfaces = hostSurfaces(room);
  for (const c of curios) {
    if (c.h === 0 && hostsOf(surfaces, c).length === 0) {
      // Only a ball stands on the bare floor.
      expect(CURIO_CATALOGUE[c.kind].slot, `${name} ${c.kind}`).toBe("ball");
      continue;
    }
    expect(
      hostsOf(surfaces, c).some((s) => curioFits(room, c, s)),
      `${name} ${c.kind}`,
    ).toBe(true);
  }
  for (const [i, a] of curios.entries())
    for (const b of curios.slice(i + 1))
      expect(curiosClash(a, b), `${name} ${a.kind} ${b.kind}`).toBe(false);
  expect([...curios].sort(CURIO_ORDER), name).toEqual(curios);
}

describe("the curio pass (C6, C7, C9, C10, C12)", () => {
  it("draws each slot at its share", () => {
    const room = generateRoom(CANNED_WORKSHOP);
    const n = 5000;
    const counts = { retro: 0, gear: 0, ball: 0, under: 0, floor: 0 };
    for (let i = 0; i < n; i++) {
      const d = curioDraws(reseed(room, "draws", i));
      if (d.retro.take) counts.retro++;
      if (d.gear.take) counts.gear++;
      if (d.ball.take) counts.ball++;
      if (d.under.take) counts.under++;
      if (d.ball.floor) counts.floor++;
    }
    expect(within(counts.retro, n, RETRO_SHARE), `retro ${counts.retro}`).toBe(
      true,
    );
    expect(within(counts.gear, n, GEAR_SHARE), `gear ${counts.gear}`).toBe(
      true,
    );
    expect(within(counts.ball, n, BALL_SHARE), `ball ${counts.ball}`).toBe(
      true,
    );
    expect(within(counts.under, n, UNDER_SHARE), `under ${counts.under}`).toBe(
      true,
    );
    expect(within(counts.floor, n, BALL_FLOOR), `floor ${counts.floor}`).toBe(
      true,
    );
  });

  it("places one curio per forced slot in every room that has a host for it, and keeps every invariant", () => {
    for (const { name, room } of ROOMS) {
      const b = base(room);
      const got = placeCurios(b, ALL_FORCED);
      const slots = new Set(got.map((c) => CURIO_CATALOGUE[c.kind].slot));
      for (const slot of ["retro", "gear", "under"] as const)
        expect(slots.has(slot), `${name} ${slot}`).toBe(true);
      expectInvariants(b, got, name);
      expect(placeCurios(b, ALL_FORCED), name).toEqual(got);
    }
  });

  it("places the floor ball in a free corner only, and falls back to the surfaces", () => {
    const draws: CurioDraws = {
      ...ALL_FORCED,
      ball: { take: true, roll: 0.25, floor: true },
    };
    // Corners with both walls bare are rare (a wall prop takes 2/3 of the
    // free edges), and every archetype and condition of one place shares
    // its wall props, so the rooms vary by permalink.
    const rooms = [
      ...ROOMS,
      ...Array.from({ length: 16 }, (_, i) =>
        Object.values(ARCHETYPE_TYPES).map((type) => ({
          name: `pipe-shop-${String(i)} ${type}`,
          room: generateRoom({
            ...CANNED_WORKSHOP,
            permalink: `pipe-shop-${String(i)}`,
            type,
          }),
        })),
      ).flat(),
    ];
    let floorBalls = 0;
    for (const { name, room } of rooms) {
      const b = base(room);
      const got = placeCurios(b, draws);
      expectInvariants(b, got, name);
      const balls = got.filter((c) => CURIO_CATALOGUE[c.kind].slot === "ball");
      // Every room has a desk, so the ball always lands somewhere.
      expect(balls.length, name).toBe(1);
      const sites = dressingSites(b);
      const hall = room.hall;
      for (const c of balls) {
        if (c.h !== 0) continue; // fell back to a surface
        floorBalls++;
        const x = c.x * CELL;
        const z = c.y * CELL;
        const nearX = Math.min(x - hall.x0 * CELL, hall.x1 * CELL - x);
        const nearZ = Math.min(z - hall.y0 * CELL, hall.y1 * CELL - z);
        expect(nearX, name).toBeLessThanOrEqual(0.2);
        expect(nearZ, name).toBeLessThanOrEqual(0.2);
        const box = curioBox(c);
        for (const lane of sites.lanes)
          expect(overlaps(box, lane), name).toBe(false);
        for (const t of sites.taken) expect(overlaps(box, t), name).toBe(false);
        for (const p of room.props) {
          const f = propFootprint(p);
          if (f !== null)
            expect(overlaps(box, f), `${name} ${p.kind}`).toBe(false);
        }
        for (const h of room.heroes)
          expect(overlaps(box, heroFootprint(h)), `${name} ${h.kind}`).toBe(
            false,
          );
      }
    }
    expect(floorBalls).toBeGreaterThan(0);
  });

  it("keeps every curio under its clear and the ceiling", () => {
    const hub = generateRoom({ ...CANNED_HUB, type: ARCHETYPE_TYPES.archive });
    expect(hub.archetype).toBe("archive");
    let swords = 0;
    for (let i = 0; i < 40; i++) {
      const b = { ...reseed(hub, "low", i), ceiling: 3.0 };
      const own = curioDraws(b);
      const got = placeCurios(b, {
        ...own,
        gear: { take: true, roll: 0, kind: "light-sword" },
      });
      expectInvariants(b, got, `low ${String(i)}`);
      const surfaces = hostSurfaces(b);
      for (const c of got) {
        const top = curioSize(c).top;
        expect(c.h + top, c.kind).toBeLessThanOrEqual(3.0 - CURIO_CEILING_GAP);
        const hosts = hostsOf(surfaces, c);
        if (c.h > 0) expect(hosts.length, c.kind).toBeGreaterThan(0);
        for (const s of hosts.filter((s) => curioFits(b, c, s)))
          expect(top, `${c.kind} on ${s.host}`).toBeLessThanOrEqual(s.clear);
        if (c.kind !== "light-sword") continue;
        swords++;
        if (c.variant === 0) continue;
        // No upright sword on a shelf level with another level above it.
        expect(
          hosts.some(
            (s) => s.host === "prop:storage-shelf" && s.clear < OPEN_CLEAR,
          ),
          `upright sword at ${String(c.h)}`,
        ).toBe(false);
      }
    }
    expect(swords).toBeGreaterThan(0);
  });

  it("leaves a slot empty where nothing fits, and never throws", () => {
    for (const [name, place] of Object.entries(DEGENERATE_PLACES)) {
      const b = base(generateRoom(place));
      let got: Curio[] = [];
      expect(() => (got = placeCurios(b, ALL_FORCED)), name).not.toThrow();
      expectInvariants(b, got, name);
    }
    const archive = base(
      generateRoom({ ...CANNED_WORKSHOP, type: ARCHETYPE_TYPES.archive }),
    );
    expect(
      placeCurios(archive, {
        retro: { take: true, roll: 0, kind: "beige-laptop" },
        gear: OFF,
        ball: { ...OFF, floor: false },
        under: OFF,
      }),
    ).toEqual([]);
    const bridge = base(generateRoom(CANNED_BRIDGE));
    const got = placeCurios(bridge, {
      ...ALL_FORCED,
      retro: { take: true, roll: 0.5, kind: "beige-laptop" },
    });
    expect(got.some((c) => c.kind === "beige-laptop")).toBe(false);
    const slots = new Set(got.map((c) => CURIO_CATALOGUE[c.kind].slot));
    for (const slot of ["gear", "ball", "under"] as const)
      expect(slots.has(slot), slot).toBe(true);
    expectInvariants(bridge, got, "bridge");
  });

  it("changes no blocker and no reach", () => {
    for (const { name, room } of ROOMS) {
      const b = base(room);
      const curios = placeCurios(b, ALL_FORCED);
      const withCurios: RoomSpec = { ...b, curios };
      expect(blockersFor(withCurios), name).toEqual(
        blockersFor({ ...withCurios, curios: [] }),
      );
      const surfaces = hostSurfaces(b);
      for (const c of curios) {
        const hosts = hostsOf(surfaces, c).filter((s) => curioFits(b, c, s));
        if (c.h === 0 && hosts.length === 0) continue; // a floor ball
        expect(hosts.length, `${name} ${c.kind}`).toBeGreaterThan(0);
        expect(
          hosts.some((s) => {
            const host = s.anchorOf;
            const foot = room.fixtures.includes(host as never)
              ? footprintOf(host as never)
              : room.decor.includes(host as never)
                ? decorFootprint(host as never)
                : room.props.includes(host as never)
                  ? propFootprint(host as never)
                  : heroFootprint(host as never);
            return foot !== null && contains(foot, curioBox(c));
          }),
          `${name} ${c.kind}`,
        ).toBe(true);
      }
    }
  });

  it("rolls only over the kinds that fit (C6)", () => {
    const retroOnly = (roll: number): CurioDraws => ({
      retro: { take: true, roll },
      gear: OFF,
      ball: { ...OFF, floor: false },
      under: OFF,
    });
    const laptops = (rooms: readonly Made[], n: number) => {
      let placed = 0;
      let laptop = 0;
      for (let i = 0; i < n; i++) {
        const made = rooms[i % rooms.length];
        if (made === undefined) throw new Error("rooms");
        const b = reseed(made.room, "fit", i);
        const got = placeCurios(b, retroOnly(curioDraws(b).retro.roll));
        placed += got.length;
        laptop += got.filter((c) => c.kind === "beige-laptop").length;
      }
      return { placed, laptop };
    };
    const wide = WORKSHOPS.filter(
      (m) => m.archetype === "council" || m.archetype === "lab",
    );
    const a = laptops(wide, 600);
    expect(a.placed).toBe(600);
    expect(within(a.laptop, a.placed, 4 / 12), `laptops ${a.laptop}`).toBe(
      true,
    );
    const archive = laptops(
      WORKSHOPS.filter((m) => m.archetype === "archive"),
      600,
    );
    expect(archive.placed).toBeGreaterThan(0);
    expect(archive.laptop).toBe(0);
  });

  it("the laptop lands now and then, and a ball about 1 room in 30", () => {
    const wide = WORKSHOPS.filter(
      (m) => m.archetype === "council" || m.archetype === "lab",
    );
    let laptops = 0;
    for (let i = 0; i < 600; i++) {
      const made = wide[i % wide.length];
      if (made === undefined) throw new Error("rooms");
      const got = placeCurios(reseed(made.room, "laptop", i));
      laptops += got.filter((c) => c.kind === "beige-laptop").length;
    }
    expect(
      within(laptops, 600, (RETRO_SHARE * 4) / 12),
      `laptops ${laptops}`,
    ).toBe(true);
    let balls = 0;
    for (let i = 0; i < 1500; i++) {
      const made = WORKSHOPS[i % WORKSHOPS.length];
      if (made === undefined) throw new Error("rooms");
      const got = placeCurios(reseed(made.room, "ball", i));
      balls += got.filter(
        (c) => CURIO_CATALOGUE[c.kind].slot === "ball",
      ).length;
    }
    expect(within(balls, 1500, BALL_SHARE), `balls ${balls}`).toBe(true);
  });

  it("puts a curio where it is told on a surface, and throws where it cannot stand (curioOn)", () => {
    const surfaces = hostSurfaces(base(generateRoom(CANNED_WORKSHOP)));
    const desk = surfaces.find((s) => s.cls === "desk");
    const under = surfaces.find((s) => s.cls === "under");
    if (desk === undefined || under === undefined) throw new Error("surfaces");
    const c = curioOn(desk, "pocket-console", 0, 0, 1, 7);
    expect(c.h).toBe(desk.h);
    expect(c.turn).toBe(desk.turn);
    expect(c.seed).toBe(7);
    expect(contains(desk.box, curioBox(c))).toBe(true);
    // u 0 hugs the low x edge by the margin, v 1 the high z edge.
    expect(curioBox(c).x0).toBeCloseTo(desk.box.x0 + 0.02, 2);
    expect(curioBox(c).z1).toBeCloseTo(desk.box.z1 - 0.02, 2);
    expect(() => curioOn(under, "pocket-console", 0, 0.5, 0.5, 1)).toThrow();
    expect(() => curioOn(desk, "beige-laptop", 0, 0.5, 0.5, 1)).toThrow();
  });

  it("pins one surfaced hero and its curios exactly (C11)", () => {
    const place = { ...CANNED_WORKSHOP, type: ARCHETYPE_TYPES.council };
    const built = generateRoom(place);
    expect(built.archetype).toBe("council");
    const pool = HERO_POOLS.council;
    const at = pool.findIndex(([k]) => k === "mess-table");
    const entry = pool[at];
    if (entry === undefined)
      throw new Error("the council pools the mess table");
    const before = pool.slice(0, at).reduce((sum, [, w]) => sum + w, 0);
    const total = pool.reduce((sum, [, w]) => sum + w, 0);
    const picks = Array.from({ length: heroCap(built.hall) }, () => ({
      take: false,
      roll: 0,
    }));
    picks[0] = { take: true, roll: (before + entry[1] / 2) / total };
    const draws: HeroDraws = { slab: false, turret: false, picks };
    const heroes = placeHeroes(built, draws, dressingSites(built));
    expect(heroes.map((h) => h.kind)).toEqual(["mess-table"]);
    const withHeroes = { ...base(built), heroes };
    const dressed: CurioBase = { ...withHeroes, props: dressRoom(withHeroes) };
    const forced: CurioDraws = {
      retro: { take: true, roll: 0, kind: "beige-laptop" },
      gear: { take: true, roll: 0, kind: "light-sword" },
      ball: { take: true, roll: 0, floor: false, kind: "star-ball" },
      under: { take: true, roll: 0, kind: "trap-box" },
    };
    const got = placeCurios(dressed, forced);
    expect(got).toEqual(PINNED);
    expectInvariants(dressed, got, "pinned");
    const { room } = roomWithForcedHero(place, "mess-table");
    expect(placeCurios(base(room), forced)).toEqual(PINNED);
  });

  it("pins a curio on a hero top exactly (C11)", () => {
    // The engineering workshop with the mess table forced in, through the
    // dev seam: the laptop's first fitting candidate is the table's top.
    const { room, placed } = roomWithForcedHero(CANNED_WORKSHOP, "mess-table");
    expect(placed).toBe("mess-table");
    const b = base(room);
    const got = placeCurios(b, {
      retro: { take: true, roll: 0, kind: "beige-laptop" },
      gear: { take: true, roll: 0, kind: "light-sword" },
      ball: { take: true, roll: 0, floor: false, kind: "star-ball" },
      under: { take: true, roll: 0, kind: "trap-box" },
    });
    expect(got).toEqual(PINNED_ON_HERO);
    expectInvariants(b, got, "on hero");
    const laptop = got.find((c) => c.kind === "beige-laptop");
    if (laptop === undefined) throw new Error("no laptop");
    const hosts = hostsOf(hostSurfaces(b), laptop).filter((s) =>
      curioFits(b, laptop, s),
    );
    expect(hosts.map((s) => s.host)).toEqual(["hero:mess-table"]);
  });
});

/**
 * The forced council workshop's curios (C11), written from the pass's
 * output after a look at each host: the upright green sword and the
 * laptop on two places of the round table (its places 2 and 1, the laptop
 * an exact fit, facing the table's front), the star ball on the desk end
 * of the west terminal at row 8, and the trap in the knee space of the
 * west terminal at row 10, facing out of the wall. The mess table stands
 * at (3.5, 7.5) turned 3; no curio's first fitting candidate is on it, by
 * the seed order of C9.
 */
const PINNED: Curio[] = [
  {
    kind: "light-sword",
    variant: 2,
    x: 6.486,
    y: 5.624,
    h: 0.78,
    turn: 3,
    seed: 1859750762012304,
  },
  {
    kind: "beige-laptop",
    variant: 0,
    x: 6.113,
    y: 5.998,
    h: 0.78,
    turn: 0,
    seed: 574460450533092,
  },
  {
    kind: "star-ball",
    variant: 0,
    x: 0.203,
    y: 8.787,
    h: 0.78,
    turn: 0,
    seed: 2159271467005853,
  },
  {
    kind: "trap-box",
    variant: 0,
    x: 0.123,
    y: 10.557,
    h: 0,
    turn: 1,
    seed: 1546703114626784,
  },
];

/**
 * The forced engineering workshop's curios with the mess table at (3.5,
 * 7.5) turned 3: the laptop on the table's top (h 0.76), turned to face
 * the table's back (a free host's `fixed` curio faces front or back), the
 * star ball and the lying sword on the two desk ends of the west terminal
 * at row 8, and the trap in the knee space of the west terminal at row
 * 10.
 */
const PINNED_ON_HERO: Curio[] = [
  {
    kind: "beige-laptop",
    variant: 0,
    x: 3.556,
    y: 6.698,
    h: 0.76,
    turn: 1,
    seed: 6857075118340509,
  },
  {
    kind: "star-ball",
    variant: 0,
    x: 0.204,
    y: 8.211,
    h: 0.78,
    turn: 0,
    seed: 2478011078854370,
  },
  {
    kind: "light-sword",
    variant: 0,
    x: 0.174,
    y: 8.785,
    h: 0.78,
    turn: 0,
    seed: 2159271467005853,
  },
  {
    kind: "trap-box",
    variant: 0,
    x: 0.123,
    y: 10.557,
    h: 0,
    turn: 1,
    seed: 1546703114626784,
  },
];
