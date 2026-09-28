/**
 * The hangar deck as a space (M3 C13 to C18): which folder is a hangar,
 * its layout (the bay door, the pads, the gantries, the doors on the
 * other walls), the dressing and the decals round the pads, the pad hero
 * seam and the golden.
 */
import { describe, expect, it } from "vitest";

import { seedFor } from "../core/seed";
import { CANNED_DECK, CANNED_HANGAR } from "./canned";
import { curioBox } from "./curios";
import { bayNumber, deckNumber } from "./decals";
import { generateDeck } from "./deck";
import { footprint, heroFootprint, propFootprint } from "./footprints";
import hangarGolden from "./golden/hangar.json?raw";
import {
  HANGAR_CEILING,
  HANGAR_DEPTH,
  HANGAR_WIDTH,
  hangarHeroes,
  hangarLayout,
  isHangar,
  withPadHeroes,
} from "./hangar";
import hangarSource from "./hangar.ts?raw";
import {
  gantryBeams,
  gantryLegEdges,
  gantryLegs,
  padBox,
  padStencilSpots,
} from "./hangarShape";
import hangarShapeSource from "./hangarShape.ts?raw";
import { heroEdges, isTallHero } from "./heroes";
import { LANE_DEPTH, LANE_WIDTH, PROP_CATALOGUE, SPAN_CELLS } from "./props";
import { dressingSites, edgeKey, edgeOf, overlaps, spanBox } from "./sites";
import type { Box, Decal, Hero, RoomSpec } from "./types";
import { CELL } from "./units";

/** A door's way lane, as `dressingSites` gives a door. */
const WAY_LANE = { along: LANE_WIDTH, out: LANE_DEPTH };

/** A floor decal's box in metres, as `placeDecals` measures it. */
function floorBox(d: Decal): Box {
  const sideways = d.turn % 2 === 1;
  const hx = (sideways ? d.length : d.width) / 2;
  const hz = (sideways ? d.width : d.length) / 2;
  return {
    x0: d.x * CELL - hx,
    x1: d.x * CELL + hx,
    z0: d.y * CELL - hz,
    z1: d.y * CELL + hz,
  };
}

/** The hangar's floor stencils that carry a pad's letter (1 or 2). */
function padStencils(room: RoomSpec): Decal[] {
  return room.decals.filter(
    (d) =>
      d.kind === "stencil" &&
      d.on === "floor" &&
      d.stencil !== undefined &&
      d.stencil.letter > 0,
  );
}

/** The hangar golden's room: `CANNED_HANGAR`, section 0. */
function hangarRoom(): RoomSpec {
  return generateDeck(CANNED_HANGAR, 0);
}

describe("the hangar deck (M3 C13 to C18)", () => {
  it("makes one folder in five a hangar, never the root", () => {
    // Mutation caught: the modulus changed, the root allowed, the raw
    // folder hashed instead of its slug.
    const names = Array.from(
      { length: 2000 },
      (_, i) => `Wing ${String(i)}/Store Room`,
    );
    expect(names.length).toBe(2000);
    const share =
      names.filter((n) => isHangar("station", n)).length / names.length;
    expect(share).toBeGreaterThanOrEqual(0.17);
    expect(share).toBeLessThanOrEqual(0.23);

    // The root: some of these domains hash their empty folder to a
    // hangar, so a root let through turns this red.
    const domains = Array.from({ length: 50 }, (_, i) => `d${String(i)}`);
    expect(domains.some((d) => seedFor("hangar", d, "") % 5 === 0)).toBe(true);
    for (const d of domains) expect(isHangar(d, ""), d).toBe(false);

    expect(isHangar("station", "cargo/Flight Deck")).toBe(
      isHangar("station", "cargo/Flight Deck"),
    );
    expect(isHangar(CANNED_DECK.domain, CANNED_DECK.folder)).toBe(false);
    expect(isHangar(CANNED_HANGAR.domain, CANNED_HANGAR.folder)).toBe(true);
    // The canned hangar's raw name is no hangar: only its slug is, so a
    // hash of the raw folder turns the line above red.
    expect(
      seedFor("hangar", CANNED_HANGAR.domain, CANNED_HANGAR.folder) % 5,
    ).not.toBe(0);
  });

  it("lays out the hangar: bay door north, two pads, gantries, doors on the other walls", () => {
    // Mutation caught: the leg edges left in the slot list, a door under
    // the bay door.
    const room = hangarRoom();
    expect([room.width, room.depth]).toEqual([HANGAR_WIDTH, HANGAR_DEPTH]);
    expect([room.width, room.depth]).toEqual([20, 16]);
    expect(room.ceiling).toBe(HANGAR_CEILING);
    expect(room.ceiling).toBe(9);
    expect(room.entrance).toEqual({ x: 10, y: 15 });
    expect(room.space).toBe("hangar");
    expect(room.grid.every((row) => row === ".".repeat(20))).toBe(true);
    const hangar = room.hangar;
    if (hangar === undefined) throw new Error("no hangar");
    expect(
      hangar.pads.map(({ x0, y0, x1, y1 }) => ({ x0, y0, x1, y1 })),
    ).toEqual([
      { x0: 2, y0: 4, x1: 9, y1: 11 },
      { x0: 11, y0: 4, x1: 18, y1: 11 },
    ]);
    for (const p of hangar.pads)
      expect(p.envelope).toEqual({ w: 13, l: 13, h: 7.5 });
    expect(hangar.bayDoor).toEqual({ x0: 5, x1: 15, h: 7 });
    expect(hangar.gantries).toEqual([
      { y: 3, h: 7.5 },
      { y: 12, h: 7.5 },
    ]);

    const legEdges = gantryLegEdges(room);
    expect(legEdges.map(edgeKey).sort()).toEqual(
      ["0,3,w", "19,3,e", "0,12,w", "19,12,e"]
        .map((k) => {
          const [x, y, side] = k.split(",");
          return edgeKey({
            x: Number(x),
            y: Number(y),
            side: side as "w" | "e",
          });
        })
        .sort(),
    );
    const legKeys = new Set(legEdges.map(edgeKey));

    // C15's count: every wall slot less the leg edges, the entrance edge
    // and the bay door's span leaves 28, however many rows ask.
    const free = hangarLayout({ rows: 40, seed: room.seed }).doorSlots;
    expect(free.length).toBe(28);
    expect(hangarLayout({ rows: 24, seed: room.seed }).doorSlots).toEqual(
      free.slice(0, 24),
    );
    for (const e of free) {
      const at = edgeKey(e);
      expect(
        e.side === "n" && e.x >= hangar.bayDoor.x0 && e.x < hangar.bayDoor.x1,
        at,
      ).toBe(false);
      expect(legKeys.has(at), at).toBe(false);
      expect(at).not.toBe(edgeKey({ ...room.entrance, side: "s" }));
    }
    const legs = gantryLegs(room);
    expect(legs.length).toBe(4);

    const doors = room.fixtures.filter((f) => f.kind === "door");
    expect(doors.length).toBe(24);
    expect(new Set(doors.map((d) => edgeKey(d.slot))).size).toBe(24);
    expect(doors[0]?.slot).toEqual({ x: 0, y: 14, side: "w" });
    for (const d of doors) {
      const at = edgeKey(d.slot);
      expect(
        d.slot.side === "n" &&
          d.slot.x >= hangar.bayDoor.x0 &&
          d.slot.x < hangar.bayDoor.x1,
        at,
      ).toBe(false);
      expect(at).not.toBe(edgeKey({ ...room.entrance, side: "s" }));
      expect(legKeys.has(at), at).toBe(false);
      const lane = footprint(d.slot, WAY_LANE);
      for (const leg of legs) expect(overlaps(leg, lane), at).toBe(false);
    }
    expect(room.fixtures.find((f) => f.kind === "lift")?.slot).toEqual({
      ...room.entrance,
      side: "s",
    });
    expect(room.fixtures.find((f) => f.kind === "screen")?.slot).toEqual({
      x: 4,
      y: 0,
      side: "n",
    });
    // C19: every zone lit at 224, steady.
    expect(room.lights.length).toBeGreaterThan(0);
    for (const z of room.lights) {
      expect(z.level).toBe(224);
      expect(z.special).toBe("steady");
    }
  });

  it("keeps the pads clear", () => {
    // Mutation caught: `sites.ts` not reading the pads, `placeDecals` not
    // laying the pad stencils.
    const room = hangarRoom();
    const hangar = room.hangar;
    if (hangar === undefined) throw new Error("no hangar");
    const pads = hangar.pads.map(padBox);
    expect(pads.length).toBe(2);
    expect(room.props.length).toBeGreaterThan(0);
    expect(room.heroes.length).toBeGreaterThan(0);
    expect(room.curios.length).toBeGreaterThan(0);
    for (const pad of pads) {
      for (const p of room.props) {
        const box = propFootprint(p);
        if (box !== null) expect(overlaps(box, pad), p.kind).toBe(false);
      }
      for (const h of room.heroes)
        expect(overlaps(heroFootprint(h), pad), h.kind).toBe(false);
      for (const c of room.curios)
        expect(overlaps(curioBox(c), pad), c.kind).toBe(false);
    }

    const stencils = padStencils(room);
    expect(stencils.map((d) => d.stencil)).toEqual([
      {
        deck: deckNumber(room.domain, room.permalink),
        bay: bayNumber(room.domain, room.permalink),
        letter: 1,
        lines: 1,
      },
      {
        deck: deckNumber(room.domain, room.permalink),
        bay: bayNumber(room.domain, room.permalink),
        letter: 2,
        lines: 1,
      },
    ]);
    stencils.forEach((d, i) => {
      const pad = pads[i];
      if (pad === undefined) throw new Error("no pad");
      const box = floorBox(d);
      expect(d.turn).toBe(0);
      expect(box.x0 >= pad.x0 && box.x1 <= pad.x1).toBe(true);
      expect(box.z0 >= pad.z0 && box.z1 <= pad.z1).toBe(true);
    });
    expect(padStencilSpots(hangar).map((s) => s.letter)).toEqual([1, 2]);
  });

  it("hangs nothing on the bay door or a gantry leg's wall edge", () => {
    // Mutation caught: the bay door's span or the leg edges left in the
    // free edges (a pipe run, a sign or a smear across the bay door).
    const room = hangarRoom();
    const hangar = room.hangar;
    if (hangar === undefined) throw new Error("no hangar");
    const kept = [
      ...gantryLegEdges(room),
      ...Array.from(
        { length: hangar.bayDoor.x1 - hangar.bayDoor.x0 },
        (_, i) => ({ x: hangar.bayDoor.x0 + i, y: 0, side: "n" as const }),
      ),
    ].map(edgeKey);
    expect(kept.length).toBe(14);
    const sites = dressingSites(room);
    for (const k of kept) {
      expect(sites.noRun.has(k), k).toBe(true);
      expect(sites.free.has(k), k).toBe(false);
    }
    const keptSet = new Set(kept);
    expect(
      room.props.filter((p) => p.anchor === "wall").length,
    ).toBeGreaterThan(0);
    expect(room.decals.filter((d) => d.on === "wall").length).toBeGreaterThan(
      0,
    );
    expect(room.heroes.flatMap(heroEdges).length).toBeGreaterThan(0);
    for (const p of room.props)
      if (p.anchor === "wall")
        expect(keptSet.has(edgeKey(edgeOf(p))), p.kind).toBe(false);
    for (const d of room.decals)
      if (d.on === "wall")
        expect(keptSet.has(edgeKey(edgeOf(d))), d.kind).toBe(false);
    for (const h of room.heroes)
      for (const e of heroEdges(h))
        expect(keptSet.has(edgeKey(e)), h.kind).toBe(false);
  });

  it("hangs no ceiling span across or along a gantry", () => {
    // Mutation caught: the span lines not keeping off the gantries' beams
    // (a duct run through a truss or its catwalk).
    const folders: string[] = [];
    for (let f = 0; folders.length < 20; f++)
      if (isHangar("station", `bay${String(f)}`))
        folders.push(`bay${String(f)}`);
    const rooms = [
      hangarRoom(),
      ...folders.map((folder) =>
        generateDeck(
          {
            domain: "station",
            folder,
            rows: [],
            subfolders: [],
            total: 0,
            truncated: false,
          },
          0,
        ),
      ),
    ];
    expect(rooms.length).toBe(21);
    let spans = 0;
    for (const room of rooms) {
      expect(room.space).toBe("hangar");
      const beams = gantryBeams(room);
      expect(beams.length).toBe(2);
      for (const p of room.props) {
        if (!PROP_CATALOGUE[p.kind].span) continue;
        spans++;
        const box =
          p.turn === 0
            ? spanBox("x", { x: p.x - SPAN_CELLS / 2, y: p.y - 0.5 })
            : spanBox("y", { x: p.x - 0.5, y: p.y - SPAN_CELLS / 2 });
        for (const b of beams)
          expect(overlaps(box, b), `${room.permalink} ${p.kind}`).toBe(false);
      }
    }
    expect(spans).toBeGreaterThan(0);
  });

  it("stands heroes on the pads through the seam (C18)", () => {
    // Mutation caught: the pad stencils appended outside `placeDecals`
    // (they vanish on the re-run), the heroes re-sorted away.
    const room = hangarRoom();
    const hangar = room.hangar;
    if (hangar === undefined) throw new Error("no hangar");
    expect(hangarHeroes(room)).toEqual([]);
    expect(withPadHeroes(room, room.heroes)).toEqual(room);

    expect(isTallHero("black-slab")).toBe(true);
    const onPads: Hero[] = hangar.pads.map((p) => ({
      kind: "black-slab",
      variant: 0,
      x: (p.x0 + p.x1) / 2,
      y: (p.y0 + p.y1) / 2,
      turn: 0,
      seed: p.seed,
    }));
    expect(onPads.length).toBe(2);
    const fitted = withPadHeroes(room, [...room.heroes, ...onPads]);
    expect(fitted.heroes.length).toBe(room.heroes.length + 2);
    for (const h of onPads) expect(fitted.heroes).toContainEqual(h);
    expect(fitted.props.length).toBeGreaterThan(0);
    expect(fitted.curios.length).toBeGreaterThan(0);
    expect(
      fitted.decals.filter((d) => d.on === "floor").length,
    ).toBeGreaterThan(0);
    for (const h of onPads) {
      const foot = heroFootprint(h);
      for (const p of fitted.props) {
        const box = propFootprint(p);
        if (box !== null) expect(overlaps(box, foot), p.kind).toBe(false);
      }
      for (const c of fitted.curios)
        expect(overlaps(curioBox(c), foot), c.kind).toBe(false);
      for (const d of fitted.decals)
        if (d.on === "floor")
          expect(overlaps(floorBox(d), foot), d.kind).toBe(false);
    }
    expect(padStencils(fitted)).toEqual(padStencils(room));
    expect(padStencils(fitted).length).toBe(2);
    // Nothing but the heroes and what is laid round them moves.
    const still = (r: RoomSpec) => ({
      ...r,
      heroes: [],
      props: [],
      curios: [],
      decals: [],
    });
    expect(still(fitted)).toEqual(still(room));
  });

  it("keeps the hangar's shape a leaf and the deck a type-only import", () => {
    // Mutation caught: `hangarShape.ts` importing anything but the types
    // and the units (a cycle through `sites.ts`), `hangar.ts` importing
    // `deck.ts` at run time (a cycle through `generateDeck`).
    const imports = [
      ...hangarShapeSource.matchAll(/\bfrom\s*["']([^"']+)["']/g),
    ].map((m) => m[1]);
    expect(imports.length).toBeGreaterThan(0);
    for (const i of imports) expect(["./types", "./units"]).toContain(i);
    expect(hangarSource).not.toMatch(
      /^import\s+(?!type\b)[^;]*from\s*["']\.\/deck["']/m,
    );
    expect(hangarSource).not.toMatch(
      /\b(?:from|import)\s*\(?\s*["'](?:\.\.\/(?:ui|render|session)(?:\/[^"']*)?|\.\/(?:move|malfunction|interact|box|arrival|station))["']/,
    );
  });

  it("builds the same hangar twice and matches the golden byte for byte", () => {
    // Mutation caught: any change to the builder's output, key order included.
    expect(hangarRoom()).toEqual(hangarRoom());
    const text = JSON.stringify(hangarRoom(), null, 2) + "\n";
    expect(Object.keys(hangarRoom()).slice(3, 6)).toEqual([
      "permalink",
      "space",
      "hangar",
    ]);
    expect(text).toBe(hangarGolden);
  });
});
