import { describe, expect, it } from "vitest";

import { LOOKS } from "../render/looks";
import { normals, positions } from "../render/modelChecks";
import { HOUSING_DEPTH } from "../render/models/doors";
import { buildPropMesh } from "../render/models/props";
import { CANNED_BRIDGE, CANNED_HUB, CANNED_WORKSHOP } from "./canned";
import {
  CRATE_FACES,
  DECAL_CAP,
  DECAL_RATES,
  DOOR_DEPTH,
  HAZARD_MACHINES,
  STREAK_SOURCES,
  bayNumber,
  deckNumber,
  placeDecals,
} from "./decals";
import decalsSource from "./decals.ts?raw";
import {
  FOOTPRINTS,
  HERO_FRONT,
  footprint,
  heroFootprint,
  propFootprint,
} from "./footprints";
import { generateRoom, withHeroes } from "./generate";
import { heroReserve } from "./heroes";
import { wallRuns } from "./layout";
import { PROP_CATALOGUE } from "./props";
import {
  dressingSites,
  edgeKey,
  edgeOf,
  fitsFloor,
  inside,
  overlaps,
  STENCIL_STRIP,
  stencilEdge,
  wallAnchor,
} from "./sites";
import type {
  Box,
  Decal,
  FloorPropKind,
  PlaceInput,
  Prop,
  PropKind,
  Rect,
  RoomSpec,
} from "./types";
import { CELL } from "./units";

const STATUSES = {
  clean: "stable",
  construction: "draft",
  dim: "deprecated",
  derelict: "archived",
} as const;
const PLACES = [CANNED_BRIDGE, CANNED_WORKSHOP, CANNED_HUB];

/** Every canned place in every condition, and `n` reseeds of the workshop and the bridge in each. */
function rooms(n: number): RoomSpec[] {
  const out: RoomSpec[] = [];
  for (const status of Object.values(STATUSES)) {
    for (const p of PLACES) out.push(generateRoom({ ...p, status }));
    for (let i = 0; i < n; i++)
      for (const p of [CANNED_WORKSHOP, CANNED_BRIDGE])
        out.push(
          generateRoom({
            ...p,
            status,
            permalink: `${p.permalink}-${String(i)}`,
          }),
        );
  }
  return out;
}

/**
 * A floor decal's box in metres: centre `(x, y) * CELL`, `width` across
 * and `length` along its turn (along z at turns 0 and 2, along x at 1 and
 * 3).
 */
function floorBox(d: Decal): Box {
  const sideways = d.turn % 2 === 1;
  const hx = (sideways ? d.length : d.width) / 2;
  const hz = (sideways ? d.width : d.length) / 2;
  const cx = d.x * CELL;
  const cz = d.y * CELL;
  return { x0: cx - hx, x1: cx + hx, z0: cz - hz, z1: cz + hz };
}

/**
 * The index of the prop a face decal lies on: the floor prop with a flat
 * face (`CRATE_FACES`) at the decal's turn whose face point, `prop +
 * HERO_FRONT[turn] * (depth / 2 - inset) / CELL`, is the decal's; -1 for
 * none.
 */
function faceProp(room: RoomSpec, d: Decal): number {
  const [fx, fz] = HERO_FRONT[d.turn] ?? [0, -1];
  return room.props.findIndex((p) => {
    if (p.anchor !== "floor" || p.turn % 4 !== d.turn) return false;
    const face = CRATE_FACES[p.kind as FloorPropKind]?.[p.variant];
    const size = FOOTPRINTS.prop[p.kind as FloorPropKind][p.variant];
    if (!face || !size) return false;
    const out = (size.depth / 2 - face.inset) / CELL;
    return (
      Math.abs(p.x + fx * out - d.x) < 1e-3 &&
      Math.abs(p.y + fz * out - d.y) < 1e-3
    );
  });
}

/** True when a box lies wholly inside a rectangle of cells. */
function within(r: Rect, b: Box): boolean {
  return (
    b.x0 >= r.x0 * CELL &&
    b.x1 <= r.x1 * CELL &&
    b.z0 >= r.y0 * CELL &&
    b.z1 <= r.y1 * CELL
  );
}

describe("decal placement (2.7 Task 10)", () => {
  it("never lays a wall decal on an edge that carries text, a fixture or a reservation (2.7 C16)", () => {
    // Mutation caught: a smear on an edge with a sign plate, a streak on an
    // edge whose prop is no source, a decal on a fixture's or a wall hero's
    // edge, a stencil moved off its empty edge, or a smear or streak that
    // runs past its own edge onto the next one (over a door frame or a
    // sign plate there).
    const all = rooms(120);
    let walls = 0;
    for (const room of all) {
      const sites = dressingSites(room);
      const reserved = heroReserve(room.heroes).edges;
      const wallProps = new Map<string, PropKind>();
      for (const p of room.props)
        if (p.anchor === "wall" && !PROP_CATALOGUE[p.kind].run)
          wallProps.set(edgeKey(edgeOf(p)), p.kind);
      for (const d of room.decals) {
        if (d.on !== "wall") continue;
        walls++;
        expect(Math.abs(d.along) + d.width / 2).toBeLessThanOrEqual(
          CELL / 2 + 1e-9,
        );
        expect(d.h).toBeGreaterThanOrEqual(0);
        const e = edgeKey(edgeOf(d));
        expect(sites.fixtureEdges.has(e)).toBe(false);
        expect(reserved.has(e)).toBe(false);
        const prop = wallProps.get(e);
        if (d.kind === "stencil") {
          expect(e).toBe(
            edgeKey({ x: room.entrance.x + 1, y: room.entrance.y, side: "s" }),
          );
          expect(prop).toBeUndefined();
        } else {
          expect(sites.noRun.has(e)).toBe(false);
          if (prop !== undefined) {
            expect(["streak", "rust"]).toContain(d.kind);
            expect(["vent-grille", "pipe-riser"]).toContain(prop); // the sources that carry no text (C16)
          }
        }
      }
    }
    expect(walls).toBeGreaterThan(200);
  }, 30_000);

  it("keeps decals off every footprint and leaves out the stencil it has no wall for (Review Focus 4)", () => {
    // Mutation caught: a floor stain under a crate, a scaffold, a hero or
    // another decal, or a wall stencil on an edge that does not exist.
    const cramped: PlaceInput = {
      ...CANNED_BRIDGE,
      permalink: "bare",
      content: "",
      tags: [],
      relations: [],
      links: [],
      inbound: [],
      inboundTotal: 0,
      observations: [],
    };
    const list = [
      ...rooms(40),
      generateRoom(cramped),
      generateRoom({ ...CANNED_WORKSHOP, status: "draft" }),
    ];
    let floors = 0;
    for (const room of list) {
      const sites = dressingSites(room);
      const solid = [
        ...sites.taken,
        ...room.props.map(propFootprint).filter((b): b is Box => b !== null),
        ...room.heroes.map((h) => heroFootprint(h)),
      ];
      const lying = room.decals.filter((d) => d.on === "floor");
      const flat = lying.map(floorBox);
      floors += flat.length;
      flat.forEach((b, i) => {
        // The corridor's arrow is the one floor decal outside the hall and
        // the bays (C14), which `fitsFloor` does not count as floor.
        const corridorArrow =
          lying[i]?.kind === "arrow" &&
          room.corridor !== null &&
          within(room.corridor, b);
        expect(fitsFloor(room, b) || corridorArrow).toBe(true);
        for (const s of solid) expect(overlaps(b, s)).toBe(false);
        flat.forEach((c, j) => {
          if (i !== j) expect(overlaps(b, c)).toBe(false);
        });
      });
      const wallEdges = new Set(wallRuns(room.grid).flat().map(edgeKey));
      const stencilEdge = edgeKey({
        x: room.entrance.x + 1,
        y: room.entrance.y,
        side: "s",
      });
      const wallStencil = room.decals.some(
        (d) => d.kind === "stencil" && d.on === "wall",
      );
      expect(wallStencil).toBe(
        wallEdges.has(stencilEdge) && !sites.fixtureEdges.has(stencilEdge),
      );
      const lane = room.decals.filter(
        (d) =>
          d.kind === "stencil" && d.on === "floor" && d.stencil?.letter === 0,
      );
      expect(lane.length).toBe(1);
      expect(lane[0]!.x).toBeCloseTo(room.entrance.x + 0.5, 6);
      expect([room.entrance.y - 1.0, room.entrance.y - 0.5]).toContain(
        lane[0]!.y,
      );
    }
    expect(floors).toBeGreaterThan(100);
    // A generated hall always has a wall east of its entrance, so the
    // missing wall is made by hand: one floor cell south of that edge.
    const g = generateRoom(CANNED_BRIDGE);
    const row = Array.from({ length: g.width }, (_, x) =>
      x === g.entrance.x + 1 ? "." : " ",
    ).join("");
    const open = placeDecals({
      ...g,
      depth: g.depth + 1,
      grid: [...g.grid, row],
    });
    expect(g.decals.some((d) => d.kind === "stencil" && d.on === "wall")).toBe(
      true,
    );
    expect(open.some((d) => d.kind === "stencil" && d.on === "wall")).toBe(
      false,
    );
  }, 30_000);

  it("numbers decks by folder and bays by address, always 1 to 99 (Review Focus 3)", () => {
    // Mutation caught: a root permalink not on deck 1, two permalinks of one
    // folder on two decks, or a number out of range (a three-digit stencil).
    expect(deckNumber("d", "readme")).toBe(1);
    expect(deckNumber("d", "a/b/c/d/e")).toBe(deckNumber("d", "a/b/c/d/other"));
    const links = [
      "readme",
      "a/b/c/d/e",
      "notizen/über uns",
      ...Array.from(
        { length: 3000 },
        (_, i) => `f${String(i % 97)}/p${String(i)}`,
      ),
    ];
    expect(links.length).toBeGreaterThan(0);
    for (const p of links) {
      for (const n of [deckNumber("d", p), bayNumber("d", p)]) {
        expect(Number.isInteger(n)).toBe(true);
        expect(n).toBeGreaterThanOrEqual(1);
        expect(n).toBeLessThanOrEqual(99);
      }
    }
    const decks = new Set(links.map((p) => deckNumber("d", p)));
    expect(decks.size).toBeGreaterThan(60); // 97 folders spread over 2 to 99
  });

  it("letters each bay and points each bay and the corridor home (Review Focus 1)", () => {
    // Mutation caught: a bay without its letter, a letter out of order, an
    // arrow pointing away from the hall.
    const room = generateRoom(CANNED_HUB);
    expect(room.bays.length).toBe(2);
    const letters = room.decals.filter(
      (d) =>
        d.kind === "stencil" &&
        d.on === "floor" &&
        (d.stencil?.letter ?? 0) > 0,
    );
    expect(letters.map((d) => d.stencil?.letter)).toEqual([1, 2]);
    const arrows = room.decals.filter((d) => d.kind === "arrow");
    const inBays = arrows.filter((d) =>
      room.bays.some((b) => inside(b, Math.floor(d.x), Math.floor(d.y))),
    );
    expect(inBays.length).toBe(room.bays.length);
    for (const a of inBays) expect(a.turn).toBe(3);
    const inCorridor = arrows.filter(
      (d) =>
        room.corridor !== null &&
        inside(room.corridor, Math.floor(d.x), Math.floor(d.y)),
    );
    expect(inCorridor.map((d) => d.turn)).toEqual([1]);
    expect(room.decals.length).toBeLessThanOrEqual(DECAL_CAP);
  });

  it("chevrons every heavy door and every hazard machine whose strip is clear, and nothing else", () => {
    // Mutation caught: a sliding door with chevrons, a hazard machine
    // without them, chevrons on a harmless machine, or a strip laid over a
    // footprint. The planner measured no strip ever blocked (C17), so the
    // skipped count is pinned at 0 over this sweep too.
    let skipped = 0;
    let strips = 0;
    for (const room of rooms(10)) {
      const owners = room.fixtures.filter(
        (f) =>
          (f.kind === "door" && f.style !== "sliding") ||
          (f.kind === "machine" && HAZARD_MACHINES.includes(f.machine)),
      );
      const laid = room.decals.filter(
        (d) => d.kind === "chevrons" && d.on === "floor",
      ).length;
      strips += laid;
      skipped += owners.length - laid;
      expect(laid).toBeLessThanOrEqual(owners.length);
    }
    expect(strips).toBeGreaterThan(0);
    expect(skipped).toBe(0);
  }, 30_000);

  it("scales grime and rust with condition, inside the measured bands (2.7 C15)", () => {
    // Mutation caught: a rate table read by the wrong condition, rust in a
    // clean room, or a rate far from C15's.
    const per = (status: string) => {
      let stains = 0,
        streaks = 0,
        rust = 0,
        cells = 0;
      for (let i = 0; i < 150; i++) {
        const room = generateRoom({
          ...CANNED_WORKSHOP,
          status,
          permalink: `w-${String(i)}`,
        });
        cells += room.grid
          .join("")
          .split("")
          .filter((c) => c === ".").length;
        for (const d of room.decals) {
          if (d.on === "floor" && d.kind === "grime") stains++;
          if (d.kind === "streak") streaks++;
          if (d.kind === "rust") rust++;
        }
      }
      return { stains: stains / cells, streaks, rust };
    };
    const clean = per("stable");
    const derelict = per("archived");
    expect(clean.rust).toBe(0);
    expect(derelict.rust).toBeGreaterThan(0);
    expect(derelict.stains).toBeGreaterThan(3 * clean.stains);
    // The bands: measured at 150 rooms (2026-09-28), clean 0.0156 and
    // derelict 0.1062 stains per floor cell, below C15's 0.02 and 0.14
    // since a stain whose box is not clear is left out; pinned here as
    // measured +- 30%.
    expect(clean.stains).toBeGreaterThan(0.0156 * 0.7);
    expect(clean.stains).toBeLessThan(0.0156 * 1.3);
    expect(derelict.stains).toBeGreaterThan(0.1062 * 0.7);
    expect(derelict.stains).toBeLessThan(0.1062 * 1.3);
  }, 30_000);

  it("places the same decals every time, and withHeroes keeps them for the room's own heroes", () => {
    // Mutation caught: a draw from an unseeded stream, or withHeroes that
    // forgets to lay the decals again.
    for (const p of PLACES) {
      const g = generateRoom(p);
      expect(generateRoom(p).decals).toEqual(g.decals);
      expect(withHeroes(p, g, g.heroes)).toEqual(g);
    }
  });

  it("lays the decals again when withHeroes stands other heroes", () => {
    // Mutation caught: withHeroes that keeps the room's old decals, so a
    // floor stain would lie under a hero it never saw.
    const place = { ...CANNED_WORKSHOP, status: "archived" };
    const g = generateRoom(place);
    expect(g.heroes.length).toBeGreaterThan(0);
    const bare = withHeroes(place, g, []);
    const { decals, ...rest } = bare;
    expect(decals).toEqual(placeDecals(rest));
    expect(decals).not.toEqual(g.decals);
  });

  it("lays a face decal only on a crate's flat face whose front is clear (2.7 C14, C16)", () => {
    // Mutation caught: a face decal on a face pressed against a touching
    // cluster neighbour, a taken box or a hero (the strip check dropped),
    // on a crate with no flat face or a marked crate, or off its face's
    // flat band.
    let faces = 0;
    for (const room of rooms(40)) {
      const sites = dressingSites(room);
      const heroes = room.heroes.map((h) => heroFootprint(h));
      const boxes = room.props.map(propFootprint);
      for (const d of room.decals) {
        if (d.on !== "face") continue;
        faces++;
        const [fx, fz] = HERO_FRONT[d.turn] ?? [0, -1];
        const i = faceProp(room, d);
        expect(i).toBeGreaterThanOrEqual(0);
        const prop = room.props[i]!;
        expect(prop.kind).not.toBe("marked-crate");
        const face = CRATE_FACES[prop.kind as FloorPropKind]![prop.variant]!;
        const size = FOOTPRINTS.prop[prop.kind as FloorPropKind][prop.variant]!;
        expect(d.h).toBeGreaterThanOrEqual(face.h0 - 1e-9);
        expect(d.h + d.length).toBeLessThanOrEqual(face.h1 + 1e-9);
        expect(d.width).toBeLessThanOrEqual(size.width - 2 * face.inset);
        const own = boxes[i]!;
        const strip: Box =
          fx > 0
            ? { ...own, x0: own.x1, x1: own.x1 + 0.05 }
            : fx < 0
              ? { ...own, x0: own.x0 - 0.05, x1: own.x0 }
              : fz > 0
                ? { ...own, z0: own.z1, z1: own.z1 + 0.05 }
                : { ...own, z0: own.z0 - 0.05, z1: own.z0 };
        boxes.forEach((b, j) => {
          if (j !== i && b !== null) expect(overlaps(strip, b)).toBe(false);
        });
        for (const b of [...sites.taken, ...heroes])
          expect(overlaps(strip, b)).toBe(false);
      }
    }
    expect(faces).toBeGreaterThan(100);
    // The dressing never stands a prop against a crate's face, so the
    // blocked case is made by hand: a small crate touching the front of a
    // crate that carries a face decal.
    const g = generateRoom({ ...CANNED_HUB, status: "archived" });
    const d0 = g.decals.find((d) => d.on === "face");
    expect(d0).toBeDefined();
    const host = g.props[faceProp(g, d0!)]!;
    const own = propFootprint(host)!;
    const [fx, fz] = HERO_FRONT[d0!.turn] ?? [0, -1];
    const cx =
      fx > 0 ? own.x1 + 0.4 : fx < 0 ? own.x0 - 0.4 : (own.x0 + own.x1) / 2;
    const cz =
      fz > 0 ? own.z1 + 0.4 : fz < 0 ? own.z0 - 0.4 : (own.z0 + own.z1) / 2;
    const blocker: Prop = {
      kind: "crate",
      variant: 0,
      anchor: "floor",
      x: cx / CELL,
      y: cz / CELL,
      turn: 0,
      seed: 0,
    };
    const pressed = placeDecals({ ...g, props: [...g.props, blocker] });
    expect(
      pressed.some((d) => d.on === "face" && d.x === d0!.x && d.y === d0!.y),
    ).toBe(false);
  }, 30_000);

  it("starts every door's chevron strip past the door's own depth (2.7 C14)", () => {
    // Mutation caught: a strip laid under the door's housings, sill or
    // lower leaf, or a door depth that no longer matches the models.
    expect(DOOR_DEPTH).toBe(HOUSING_DEPTH);
    let strips = 0;
    for (const room of rooms(5)) {
      for (const f of room.fixtures) {
        if (f.kind !== "door" || f.style === "sliding") continue;
        const a = wallAnchor(f.slot);
        const [fx, fz] = HERO_FRONT[a.turn] ?? [0, -1];
        const d = room.decals.find(
          (c) =>
            c.kind === "chevrons" &&
            c.on === "floor" &&
            c.turn === a.turn &&
            Math.abs((c.x - a.x) * fz - (c.y - a.y) * fx) < 1e-3,
        );
        expect(d).toBeDefined();
        const out = ((d!.x - a.x) * fx + (d!.y - a.y) * fz) * CELL;
        expect(out - d!.length / 2).toBeGreaterThan(DOOR_DEPTH);
        strips++;
      }
    }
    expect(strips).toBeGreaterThan(0);
  }, 30_000);

  it("cuts a full room down to the cap from the floor stains up, never a strip, an arrow or a stencil (2.7 C17)", () => {
    // Mutation caught: no cap at all, or a cap that drops streaks, smears,
    // faces or the never-dropped kinds before the floor stains. A derelict
    // reseed of the hub (hub-1) is the first found over the cap.
    const room = generateRoom({
      ...CANNED_HUB,
      status: "archived",
      permalink: "hub-1",
    });
    const { decals, ...rest } = room;
    const full = placeDecals(rest, Infinity);
    expect(full.length).toBeGreaterThan(DECAL_CAP);
    expect(decals.length).toBe(DECAL_CAP);
    const key = (d: Decal) => JSON.stringify(d);
    const kept = new Set(decals.map(key));
    const dropped = full.filter((d) => !kept.has(key(d)));
    expect(dropped.length).toBe(full.length - DECAL_CAP);
    for (const d of dropped) {
      expect(d.kind).toBe("grime");
      expect(d.on).toBe("floor");
    }
    expect(decals.every((d) => full.some((f) => key(f) === key(d)))).toBe(true);
  });

  it("keeps the rate table C15's, rank by rank (2.7 C15)", () => {
    // Mutation caught: a rate copied into the wrong condition's row.
    expect(DECAL_RATES).toEqual({
      clean: {
        stain: 0.02,
        streak: 0.25,
        rust: 0,
        smear: 0.1,
        face: 0.1,
        band: 0.25,
      },
      construction: {
        stain: 0.05,
        streak: 0.4,
        rust: 0,
        smear: 0.3,
        face: 0.25,
        band: 0.25,
      },
      dim: {
        stain: 0.08,
        streak: 0.7,
        rust: 0.5,
        smear: 0.5,
        face: 0.5,
        band: 0.25,
      },
      derelict: {
        stain: 0.14,
        streak: 1.0,
        rust: 0.7,
        smear: 0.8,
        face: 0.8,
        band: 0.25,
      },
    });
  });

  it("keeps decals.ts on the generator side (2.7 Global Constraints)", () => {
    // Mutation caught: decals.ts importing generate, dress, move or any of
    // render/, which would pull the renderer into the generator.
    expect(decalsSource).not.toMatch(
      /\b(?:from|import)\s*\(?\s*["'](?:\.\/(?:move|generate|dress|interact|malfunction|box|arrival)|\.\.\/render(?:\/[^"']*)?)["']/,
    );
  });
});

describe("the decal hand tables against the meshes (2.7 Task 10 Step 3b)", () => {
  const TOL = 0.005;

  it("hangs every streak from its source's lowest vertex", () => {
    // Mutation caught: a source's bottom height copied wrong from its recipe.
    const kinds = Object.entries(STREAK_SOURCES) as [
      PropKind,
      readonly number[],
    ][];
    expect(kinds.length).toBe(2);
    for (const [kind, bottoms] of kinds) {
      expect(bottoms.length).toBe(PROP_CATALOGUE[kind].variants);
      bottoms.forEach((h, v) => {
        const low = Math.min(
          ...positions(buildPropMesh(kind, v, LOOKS.aperture)).map((p) => p[1]),
        );
        expect(Math.abs(low - h), `${kind} ${String(v)}`).toBeLessThanOrEqual(
          TOL,
        );
      });
    }
  });

  it("puts every crate face band on a flat face at its footprint's edge", () => {
    // Mutation caught: a face's band or inset copied wrong, or a face given
    // to a variant whose front is not flat (the posted crate).
    const kinds = Object.entries(CRATE_FACES) as [
      FloorPropKind,
      readonly (null | { h0: number; h1: number; inset: number })[],
    ][];
    expect(kinds.length).toBe(2);
    let faces = 0;
    for (const [kind, table] of kinds) {
      expect(table.length).toBe(PROP_CATALOGUE[kind].variants);
      table.forEach((face, v) => {
        if (face === null) return;
        faces++;
        const size = FOOTPRINTS.prop[kind][v];
        expect(size).toBeDefined();
        // Built at turn 0 facing north: the front face looks along -z, at
        // z = -(depth / 2 - inset).
        const mesh = buildPropMesh(kind, v, LOOKS.aperture);
        const pos = positions(mesh);
        const nor = normals(mesh);
        const plane = -(size!.depth / 2 - face.inset);
        let lo = Infinity;
        let hi = -Infinity;
        for (let t = 0; t + 2 < pos.length; t += 3) {
          const tri = [pos[t]!, pos[t + 1]!, pos[t + 2]!];
          const n = nor[t]!;
          if (n[2] > -0.999) continue;
          if (!tri.every((p) => Math.abs(p[2] - plane) <= TOL)) continue;
          for (const p of tri) {
            lo = Math.min(lo, p[1]);
            hi = Math.max(hi, p[1]);
          }
        }
        expect(lo, `${kind} ${String(v)}`).toBeLessThanOrEqual(face.h0 + TOL);
        expect(hi, `${kind} ${String(v)}`).toBeGreaterThanOrEqual(
          face.h1 - TOL,
        );
      });
    }
    expect(faces).toBe(3);
  });

  it("sets each bay's stencil to read from the hall, its top pointing east into the bay (2.7 C19)", () => {
    // Mutation caught: a bay stencil turned to point west (turn 3), which
    // reads upside down to a player walking in from the hall.
    const room = generateRoom(CANNED_HUB);
    const bays = room.decals.filter(
      (d) => d.kind === "stencil" && (d.stencil?.letter ?? 0) > 0,
    );
    expect(bays.length).toBe(room.bays.length);
    for (const b of room.bays)
      expect(b.x0).toBeGreaterThanOrEqual(room.hall.x1);
    for (const d of bays) expect(d.turn).toBe(1);
  });

  it("keeps every floor prop and hero out of the strip in front of a wall stencil (2.7 C19)", () => {
    // Mutation caught: the strip left out of the lanes floor props keep
    // clear, so a crate stack stands in front of the stencil and hides it
    // (the canned hub at every condition).
    const list = [
      ...rooms(40),
      ...Object.values(STATUSES).map((status) =>
        generateRoom({ ...CANNED_HUB, status }),
      ),
    ];
    let stencils = 0;
    for (const room of list) {
      const walls = room.decals.filter(
        (d) => d.kind === "stencil" && d.on === "wall",
      );
      for (const d of walls) {
        stencils++;
        const e = edgeOf(d);
        expect(e).toEqual(stencilEdge(room));
        const strip = footprint(e, STENCIL_STRIP);
        for (const p of room.props) {
          const b = propFootprint(p);
          if (b !== null) expect(overlaps(strip, b)).toBe(false);
        }
        for (const h of room.heroes)
          expect(overlaps(strip, heroFootprint(h))).toBe(false);
      }
    }
    expect(stencils).toBeGreaterThan(100);
  }, 30_000);
});
