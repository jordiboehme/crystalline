import { describe, expect, it } from "vitest";

import golden from "./golden/bridge.json?raw";
import { CANNED_BRIDGE, CANNED_HUB } from "./canned";
import {
  MACHINE_KINDS,
  archetypeFor,
  conditionFor,
  doorStyleFor,
  generateRoom,
} from "./generate";
import { HALL_CAP, isFloor } from "./layout";
import type { Fixture, PlaceInput, RoomSpec } from "./types";

function kinds(fixtures: Fixture[]) {
  return fixtures.map((f) => f.kind).sort();
}

function slotKey(f: Fixture) {
  return `${f.slot.x},${f.slot.y},${f.slot.side}`;
}

describe("conditionFor", () => {
  it("follows the spec's status table", () => {
    expect(conditionFor("stable")).toBe("clean");
    expect(conditionFor("current")).toBe("clean");
    expect(conditionFor("draft")).toBe("construction");
    expect(conditionFor("POC")).toBe("construction");
    expect(conditionFor("deprecated")).toBe("dim");
    expect(conditionFor("superseded")).toBe("dim");
    expect(conditionFor("archived")).toBe("derelict");
    expect(conditionFor("Legacy")).toBe("derelict");
    expect(conditionFor("whatever")).toBe("clean");
    expect(conditionFor(null)).toBe("clean");
  });
});

describe("archetypeFor", () => {
  it("maps the known types and hashes the rest to a non-bridge archetype", () => {
    expect(archetypeFor("manifest")).toBe("bridge");
    expect(archetypeFor("decision")).toBe("council");
    expect(archetypeFor("runbook")).toBe("engineering");
    expect(archetypeFor("reference")).toBe("archive");
    expect(archetypeFor("guide")).toBe("lab");
    const odd = archetypeFor("recipe");
    expect(odd).not.toBe("bridge");
    expect(archetypeFor("recipe")).toBe(odd);
  });
});

describe("doorStyleFor", () => {
  it("grows with the target's salience", () => {
    expect(doorStyleFor(null)).toBe("sliding");
    expect(doorStyleFor(0)).toBe("sliding");
    expect(doorStyleFor(3)).toBe("sliding");
    expect(doorStyleFor(4)).toBe("bulkhead");
    expect(doorStyleFor(6)).toBe("bulkhead");
    expect(doorStyleFor(7)).toBe("blast");
    expect(doorStyleFor(10)).toBe("blast");
  });
});

describe("generateRoom on the canned bridge", () => {
  const room = generateRoom(CANNED_BRIDGE);

  it("holds milestone 1's fixtures plus a poster and a hatch", () => {
    expect(room.archetype).toBe("bridge");
    expect(room.condition).toBe("clean");
    expect(kinds(room.fixtures)).toEqual(
      [
        "door",
        "door",
        "hatch",
        "machine",
        "machine",
        "placard",
        "portal",
        "poster",
        "terminal",
        "terminal",
      ].sort(),
    );
    expect(room.dropped).toBe(0);
    expect(room.inboundMore).toBe(0);
  });

  it("gives the low-salience target a sliding door and the high one a blast door", () => {
    const doors = room.fixtures.filter((f) => f.kind === "door");
    expect(doors.map((d) => [d.address?.permalink, d.style])).toEqual([
      ["old-bridge", "sliding"],
      ["reactor-core", "blast"],
    ]);
    expect(doors.every((d) => d.sealedLabel === null)).toBe(true);
  });

  it("leads its hatch back to the engram that points here", () => {
    const hatch = room.fixtures.find((f) => f.kind === "hatch");
    expect(hatch?.kind === "hatch" && hatch.address).toEqual({
      domain: "station",
      permalink: "crew-handbook",
    });
  });

  it("numbers each terminal's section by its heading's occurrence", () => {
    const room = generateRoom({
      ...CANNED_BRIDGE,
      content: "## Notes\none\n## Log\nx\n## Notes\ntwo\n",
    });
    expect(
      room.fixtures.flatMap((f) =>
        f.kind === "terminal" ? [[f.heading, f.section]] : [],
      ),
    ).toEqual([
      ["Notes", 0],
      ["Log", 0],
      ["Notes", 1],
    ]);
  });

  it("marks the portal into another domain", () => {
    const portal = room.fixtures.find((f) => f.kind === "portal");
    expect(portal?.kind === "portal" && portal.crossDomain).toBe(true);
  });

  it("never puts two fixtures in one slot and keeps every slot on a floor cell", () => {
    expectSlotsSound(room);
  });

  it("matches the committed golden byte for byte", () => {
    expect(JSON.stringify(room, null, 2) + "\n").toBe(golden);
  });

  it("covers every floor cell with exactly one light zone", () => {
    expectLightsCoverFloor(room);
  });

  it("is a plain hall with the entrance at its south wall", () => {
    expect(room.grid).toEqual(Array.from({ length: 6 }, () => "......."));
    expect(room.hall).toEqual({ x0: 0, y0: 0, x1: 7, y1: 6 });
    expect(room.spawn).toEqual({ x: 3, y: 5, yaw: 0 });
  });
});

function expectSlotsSound(room: RoomSpec) {
  const keys = room.fixtures.map(slotKey);
  expect(new Set(keys).size).toBe(keys.length);
  for (const f of room.fixtures) {
    expect(isFloor(room.grid, f.slot.x, f.slot.y)).toBe(true);
  }
}

function expectLightsCoverFloor(room: RoomSpec) {
  const covered = new Map<string, number>();
  for (const z of room.lights) {
    let floor = 0;
    for (let y = z.y0; y < z.y1; y++)
      for (let x = z.x0; x < z.x1; x++) {
        if (!isFloor(room.grid, x, y)) continue;
        floor++;
        const k = `${x},${y}`;
        covered.set(k, (covered.get(k) ?? 0) + 1);
      }
    expect(floor).toBeGreaterThan(0);
  }
  const floorCells = room.grid
    .join("")
    .split("")
    .filter((c) => c === ".");
  expect(covered.size).toBe(floorCells.length);
  expect([...covered.values()].every((n) => n === 1)).toBe(true);
}

/** Decor footprints in cells (metres / 2), first along x at turn 0. */
const DECOR_SIZE: Record<string, [number, number]> = {
  "command-console": [1.5, 0.5],
  "captain-chair": [0.4, 0.4],
  "round-table": [1.2, 1.2],
  "council-chair": [0.35, 0.35],
  generator: [1, 1],
  "pipe-run": [0, 0],
  "shelf-row": [2, 0.4],
  "lab-island": [1.5, 0.7],
  "specimen-tank": [0.45, 0.45],
};

describe("generateRoom on the canned hub", () => {
  const room = generateRoom(CANNED_HUB);

  it("grows a corridor and bays", () => {
    expect(room.hall.x0).toBeGreaterThan(0);
    expect(room.hall.x1 - room.hall.x0).toBe(HALL_CAP);
    expect(room.width).toBeGreaterThan(room.hall.x1);
    // The corridor: floor west of the hall on its last two rows only.
    expect(isFloor(room.grid, 0, room.hall.y1 - 1)).toBe(true);
    expect(isFloor(room.grid, 0, room.hall.y1 - 3)).toBe(false);
  });

  it("puts twenty-four hatches in the corridor and names the rest", () => {
    const hatches = room.fixtures.filter((f) => f.kind === "hatch");
    expect(hatches).toHaveLength(24);
    expect(hatches.every((h) => h.slot.x < room.hall.x0)).toBe(true);
    expect(room.inboundMore).toBe(276);
    const placard = room.fixtures.find((f) => f.kind === "placard");
    expect(placard?.kind === "placard" && placard.lines).toContain(
      "+276 MORE INBOUND",
    );
  });

  it("gives every placed fixture its own slot and counts the rest as dropped", () => {
    expectSlotsSound(room);
    const wanted = 1 + 20 + 2 + 4 + 40 + 24 + 3;
    expect(room.fixtures.length + room.dropped).toBe(wanted);
  });

  it("covers exactly the floor cells with light zones", () => {
    expectLightsCoverFloor(room);
  });

  it("keeps the decor of every archetype inside the hall's interior band", () => {
    for (const type of [
      "manifest",
      "decision",
      "runbook",
      "reference",
      "guide",
    ]) {
      const r = generateRoom({ ...CANNED_HUB, type });
      expect(r.decor.length).toBeGreaterThan(0);
      const band = {
        x0: r.hall.x0 + 2,
        x1: r.hall.x1 - 2,
        y0: r.hall.y0 + 2,
        y1: r.hall.y1 - 2,
      };
      for (const d of r.decor) {
        const [w, h] = DECOR_SIZE[d.kind] ?? [0, 0];
        const [hx, hy] = d.turn % 2 === 0 ? [w / 2, h / 2] : [h / 2, w / 2];
        expect(d.x - hx).toBeGreaterThanOrEqual(band.x0 - 1e-9);
        expect(d.x + hx).toBeLessThanOrEqual(band.x1 + 1e-9);
        expect(d.y - hy).toBeGreaterThanOrEqual(band.y0 - 1e-9);
        expect(d.y + hy).toBeLessThanOrEqual(band.y1 + 1e-9);
        expect([0, 1, 2, 3]).toContain(d.turn);
      }
    }
  });

  it("seats six council chairs round the table", () => {
    const r = generateRoom({ ...CANNED_HUB, type: "decision" });
    const count = (kind: string) =>
      r.decor.filter((d) => d.kind === kind).length;
    expect(count("round-table")).toBe(1);
    expect(count("council-chair")).toBe(6);
  });

  it("leaves the aisle up from the entrance free of shelves", () => {
    const r = generateRoom({ ...CANNED_HUB, type: "reference" });
    const shelves = r.decor.filter((d) => d.kind === "shelf-row");
    expect(shelves.length).toBeGreaterThan(0);
    const x = r.spawn.x + 0.5;
    for (const s of shelves)
      expect(Math.abs(s.x - x)).toBeGreaterThanOrEqual(1);
  });

  it("drops what even four bays cannot hold", () => {
    const r = generateRoom({
      ...CANNED_HUB,
      tags: Array.from({ length: 200 }, (_, i) => `many-${i}`),
    });
    expect(r.dropped).toBeGreaterThan(0);
    expectSlotsSound(r);
    const wanted = 1 + 20 + 2 + 4 + 200 + 24 + 3;
    expect(r.fixtures.length + r.dropped).toBe(wanted);
  });
});

describe("generateRoom decor", () => {
  it("places none in a hall too small for it", () => {
    expect(generateRoom(CANNED_BRIDGE).decor).toEqual([]);
  });
});

describe("generateRoom sealed ways", () => {
  const room = generateRoom(CANNED_HUB);
  const door = (permalink: string) =>
    room.fixtures.find((f) => f.kind === "door" && f.label.includes(permalink));

  it("seals an unresolved target as not found and an unlocated one as no route", () => {
    const lost = door("deck-18");
    expect(lost?.kind === "door" && [lost.address, lost.sealedLabel]).toEqual([
      null,
      "?FILE NOT FOUND",
    ]);
    const unlocated = door("deck-19");
    expect(
      unlocated?.kind === "door" && [unlocated.address, unlocated.sealedLabel],
    ).toEqual([null, "NO ROUTE"]);
  });

  it("keeps sealedLabel null exactly where there is an address", () => {
    for (const f of room.fixtures) {
      if (f.kind === "door" || f.kind === "portal") {
        expect(f.sealedLabel === null).toBe(f.address !== null);
      }
    }
  });

  it("styles doors by target salience", () => {
    const styles = new Map(
      room.fixtures.flatMap((f) =>
        f.kind === "door" && f.address !== null
          ? [[f.address.permalink, f.style] as const]
          : [],
      ),
    );
    expect(styles.get("deck-02")).toBe("sliding");
    expect(styles.get("deck-05")).toBe("bulkhead");
    expect(styles.get("deck-09")).toBe("blast");
  });
});

describe("generateRoom determinism", () => {
  it("does not reshuffle existing machines when a tag is added", () => {
    const before = generateRoom(CANNED_BRIDGE);
    const after = generateRoom({
      ...CANNED_BRIDGE,
      tags: [...CANNED_BRIDGE.tags, "cargo"],
    });
    const machine = (r: typeof before, tag: string) =>
      r.fixtures.find((f) => f.kind === "machine" && f.tag === tag);
    for (const tag of CANNED_BRIDGE.tags) {
      const a = machine(before, tag);
      const b = machine(after, tag);
      expect(a?.kind === "machine" && a.machine).toBe(
        b?.kind === "machine" && b.machine,
      );
      expect(a?.kind === "machine" && a.hue).toBe(
        b?.kind === "machine" && b.hue,
      );
    }
  });

  it("keeps the other hatches' seeds when an inbound reference is added", () => {
    const seeds = (r: RoomSpec) =>
      new Map(
        r.fixtures.flatMap((f) =>
          f.kind === "hatch" ? [[f.address.permalink, f.seed] as const] : [],
        ),
      );
    const before = seeds(generateRoom(CANNED_BRIDGE));
    const after = seeds(
      generateRoom({
        ...CANNED_BRIDGE,
        inbound: [
          {
            address: { domain: "station", permalink: "airlock" },
            title: "Airlock",
            relType: "links_to",
          },
          ...CANNED_BRIDGE.inbound,
        ],
        inboundTotal: 2,
      }),
    );
    expect(after.has("airlock")).toBe(true);
    for (const [permalink, seed] of before) {
      expect(after.get(permalink)).toBe(seed);
    }
  });

  it("keeps the other posters' seeds when an observation category is added", () => {
    const seeds = (r: RoomSpec) =>
      new Map(
        r.fixtures.flatMap((f) =>
          f.kind === "poster" ? [[f.category, f.seed] as const] : [],
        ),
      );
    const before = seeds(generateRoom(CANNED_BRIDGE));
    const after = seeds(
      generateRoom({
        ...CANNED_BRIDGE,
        observations: [
          { category: "idea", content: "A window in the mess." },
          ...CANNED_BRIDGE.observations,
        ],
      }),
    );
    expect(after.has("idea")).toBe(true);
    for (const [category, seed] of before) {
      expect(after.get(category)).toBe(seed);
    }
  });

  it("puts up to eight hatches on the hall's south wall", () => {
    const room = generateRoom({
      ...CANNED_BRIDGE,
      inbound: Array.from({ length: 8 }, (_, i) => ({
        address: { domain: "station", permalink: `log-${i}` },
        title: `Log ${i}`,
        relType: "links_to",
      })),
      inboundTotal: 8,
    });
    const hatches = room.fixtures.filter((f) => f.kind === "hatch");
    expect(hatches).toHaveLength(8);
    for (const h of hatches) {
      expect(h.slot.side).toBe("s");
      expect(h.slot.y).toBe(room.hall.y1 - 1);
    }
  });

  it("keeps the hall's light seeds when the hatches move into a corridor", () => {
    const withHatches = (n: number) =>
      generateRoom({
        ...CANNED_BRIDGE,
        inbound: Array.from({ length: n }, (_, i) => ({
          address: { domain: "station", permalink: `log-${i}` },
          title: `Log ${i}`,
          relType: "links_to",
        })),
        inboundTotal: n,
      });
    const hallSeeds = (r: RoomSpec) =>
      new Map(
        r.lights
          .filter((z) => z.x0 >= r.hall.x0 && z.x1 <= r.hall.x1)
          .map((z) => [`${z.x0 - r.hall.x0},${z.y0}`, z.seed]),
      );
    const plain = withHatches(8);
    const corridor = withHatches(12);
    expect(plain.hall.x0).toBe(0);
    expect(corridor.hall.x0).toBeGreaterThan(0);
    // Eight hatches widen the hall for the south wall; twelve leave it
    // narrow, so compare the corners both halls have.
    const a = hallSeeds(plain);
    const b = hallSeeds(corridor);
    const shared = [...b.keys()].filter((corner) => a.has(corner));
    expect(shared.length).toBeGreaterThan(0);
    for (const corner of shared) expect(b.get(corner)).toBe(a.get(corner));
  });

  it("merges categories that differ only by surrounding spaces", () => {
    const room = generateRoom({
      ...CANNED_BRIDGE,
      observations: [
        { category: "warning", content: "one" },
        { category: " warning ", content: "two" },
      ],
    });
    expect(
      room.fixtures.flatMap((f) =>
        f.kind === "poster" ? [[f.category, f.lines]] : [],
      ),
    ).toEqual([["warning", ["one", "two"]]]);
  });

  it("gathers observations into one poster per category, notes for none", () => {
    const room = generateRoom(CANNED_HUB);
    expect(
      room.fixtures.flatMap((f) =>
        f.kind === "poster" ? [[f.category, f.lines.length]] : [],
      ),
    ).toEqual([
      ["decision", 2],
      ["warning", 1],
      ["NOTES", 1],
    ]);
  });

  it("orders hatches by address whatever order they arrive in", () => {
    const reversed = generateRoom({
      ...CANNED_HUB,
      inbound: [...CANNED_HUB.inbound].reverse(),
    });
    expect(JSON.stringify(reversed)).toBe(
      JSON.stringify(generateRoom(CANNED_HUB)),
    );
  });

  it("gives the same tag the same machine in any room", () => {
    const other: PlaceInput = {
      ...CANNED_BRIDGE,
      domain: "other",
      permalink: "elsewhere",
      tags: ["reactor"],
    };
    const a = generateRoom(CANNED_BRIDGE).fixtures.find(
      (f) => f.kind === "machine" && f.tag === "reactor",
    );
    const b = generateRoom(other).fixtures.find(
      (f) => f.kind === "machine" && f.tag === "reactor",
    );
    expect(a?.kind === "machine" && [a.machine, a.hue]).toEqual(
      b?.kind === "machine" && [b.machine, b.hue],
    );
  });

  it("orders doors by target whatever order the relations arrive in", () => {
    const reversed = generateRoom({
      ...CANNED_BRIDGE,
      relations: [...CANNED_BRIDGE.relations].reverse(),
    });
    expect(JSON.stringify(reversed)).toBe(
      JSON.stringify(generateRoom(CANNED_BRIDGE)),
    );
  });

  it("keeps the existing terminals' seeds when a section is inserted above them", () => {
    const before = generateRoom(CANNED_BRIDGE);
    const after = generateRoom({
      ...CANNED_BRIDGE,
      content: CANNED_BRIDGE.content.replace(
        "## Scope",
        "## Arrivals\n\nNew crew report here.\n\n## Scope",
      ),
    });
    const terminalSeeds = (r: typeof before) =>
      new Map(
        r.fixtures.flatMap((f) =>
          f.kind === "terminal" ? [[f.heading, f.seed] as const] : [],
        ),
      );
    const a = terminalSeeds(before);
    const b = terminalSeeds(after);
    expect(b.has("Arrivals")).toBe(true);
    for (const heading of ["Scope", "Routing"]) {
      expect(a.get(heading)).toBeDefined();
      expect(b.get(heading)).toBe(a.get(heading));
    }
  });

  it("gives two sections of the same heading different terminal seeds", () => {
    const room = generateRoom({
      ...CANNED_BRIDGE,
      content: "## Notes\none\n## Notes\ntwo\n",
    });
    const seeds = room.fixtures.flatMap((f) =>
      f.kind === "terminal" ? [f.seed] : [],
    );
    expect(seeds).toHaveLength(2);
    expect(seeds[0]).not.toBe(seeds[1]);
  });

  it("keeps a light zone's seed when the room gets wider", () => {
    const before = generateRoom(CANNED_BRIDGE);
    const extra = Array.from({ length: 6 }, (_, i) => ({
      relType: "relates_to",
      target: { domain: null, target: `wide-${i}` },
      resolved: true,
      address: null,
      targetTitle: null,
      targetSalience: null,
    }));
    const after = generateRoom({
      ...CANNED_BRIDGE,
      relations: [...CANNED_BRIDGE.relations, ...extra],
    });
    expect(after.width).toBeGreaterThan(before.width);
    const byCorner = (r: typeof before) =>
      new Map(r.lights.map((z) => [`${z.x0},${z.y0}`, z.seed]));
    const a = byCorner(before);
    const b = byCorner(after);
    for (const [corner, seed] of a) expect(b.get(corner)).toBe(seed);
  });

  it("uses every one of the twelve machine kinds for some tag", () => {
    const seen = new Set<string>();
    for (let i = 0; i < 400; i++) {
      const room = generateRoom({ ...CANNED_BRIDGE, tags: [`t${i}`] });
      const m = room.fixtures.find((f) => f.kind === "machine");
      if (m?.kind === "machine") seen.add(m.machine);
    }
    expect(seen.size).toBe(MACHINE_KINDS.length);
  });
});

describe("generateRoom limits", () => {
  it("caps the hall at 24 by 24 cells and still gives every placed fixture its own slot", () => {
    const tags = Array.from({ length: 60 }, (_, i) => `tag-${i}`);
    const relations = Array.from({ length: 30 }, (_, i) => ({
      relType: "relates_to",
      target: { domain: null, target: `t-${String(i).padStart(2, "0")}` },
      resolved: true,
      address: null,
      targetTitle: null,
      targetSalience: null,
    }));
    const room = generateRoom({ ...CANNED_BRIDGE, tags, relations });
    expect(room.hall.x1 - room.hall.x0).toBeLessThanOrEqual(HALL_CAP);
    expect(room.hall.y1 - room.hall.y0).toBeLessThanOrEqual(HALL_CAP);
    expectSlotsSound(room);
  });

  it("builds a bare room for an engram with nothing in it", () => {
    const room = generateRoom({
      ...CANNED_BRIDGE,
      type: null,
      tags: [],
      content: "",
      relations: [],
      links: [],
      inbound: [],
      inboundTotal: 0,
      observations: [],
    });
    expect(room.width).toBeGreaterThanOrEqual(5);
    expect(room.fixtures.map((f) => f.kind)).toEqual(["placard"]);
  });

  it("seals a portal whose target did not resolve", () => {
    const room = generateRoom({
      ...CANNED_BRIDGE,
      links: [
        {
          relType: null,
          target: { domain: null, target: "Nowhere" },
          resolved: false,
          address: null,
          targetTitle: null,
          targetSalience: null,
        },
      ],
    });
    const portal = room.fixtures.find((f) => f.kind === "portal");
    expect(portal?.kind === "portal" && portal.address).toBeNull();
    expect(portal?.kind === "portal" && portal.sealedLabel).toBe(
      "?FILE NOT FOUND",
    );
  });

  it("writes the validity and the inbound overflow on the placard", () => {
    const placard = (p: PlaceInput) =>
      generateRoom(p).fixtures.find((f) => f.kind === "placard");
    const plain = placard(CANNED_BRIDGE);
    expect(plain?.kind === "placard" && plain.lines).toEqual([
      "Station Crystalline",
      "TYPE manifest",
      "STATUS stable",
      "SALIENCE 7",
    ]);
    const dated = placard({
      ...CANNED_BRIDGE,
      validFrom: null,
      validTo: "2027-01-01",
      inboundTotal: 9,
    });
    expect(dated?.kind === "placard" && dated.lines.slice(4)).toEqual([
      "VALID - 2027-01-01",
      "+8 MORE INBOUND",
    ]);
    const open = placard({ ...CANNED_BRIDGE, validFrom: "2026-01-01" });
    expect(open?.kind === "placard" && open.lines[4]).toBe(
      "VALID 2026-01-01 -",
    );
  });
});
