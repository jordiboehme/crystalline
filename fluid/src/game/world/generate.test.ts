import { describe, expect, it } from "vitest";

import golden from "./golden/bridge.json?raw";
import { CANNED_BRIDGE } from "./canned";
import {
  MACHINE_KINDS,
  ROOM_CAP,
  archetypeFor,
  conditionFor,
  doorStyleFor,
  generateRoom,
} from "./generate";
import type { Fixture, PlaceInput } from "./types";

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

  it("holds what milestone 1 asks for", () => {
    expect(room.archetype).toBe("bridge");
    expect(room.condition).toBe("clean");
    expect(kinds(room.fixtures)).toEqual(
      [
        "door",
        "door",
        "machine",
        "machine",
        "placard",
        "portal",
        "terminal",
        "terminal",
      ].sort(),
    );
  });

  it("gives the low-salience target a sliding door and the high one a blast door", () => {
    const doors = room.fixtures.filter((f) => f.kind === "door");
    expect(doors.map((d) => [d.target, d.style])).toEqual([
      ["old-bridge", "sliding"],
      ["reactor-core", "blast"],
    ]);
  });

  it("marks the portal into another domain", () => {
    const portal = room.fixtures.find((f) => f.kind === "portal");
    expect(portal?.kind === "portal" && portal.crossDomain).toBe(true);
  });

  it("never puts two fixtures in one slot and keeps every slot inside the room", () => {
    const keys = room.fixtures.map(slotKey);
    expect(new Set(keys).size).toBe(keys.length);
    for (const f of room.fixtures) {
      expect(f.slot.x).toBeGreaterThanOrEqual(0);
      expect(f.slot.x).toBeLessThan(room.width);
      expect(f.slot.y).toBeGreaterThanOrEqual(0);
      expect(f.slot.y).toBeLessThan(room.depth);
    }
  });

  it("matches the committed golden byte for byte", () => {
    expect(JSON.stringify(room, null, 2) + "\n").toBe(golden);
  });

  it("covers every cell with exactly one light zone", () => {
    const covered = new Map<string, number>();
    for (const z of room.lights) {
      for (let y = z.y0; y < z.y1; y++)
        for (let x = z.x0; x < z.x1; x++) {
          const k = `${x},${y}`;
          covered.set(k, (covered.get(k) ?? 0) + 1);
        }
    }
    expect(covered.size).toBe(room.width * room.depth);
    expect([...covered.values()].every((n) => n === 1)).toBe(true);
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
  it("caps the room at 24 by 24 cells and still gives every placed fixture its own slot", () => {
    const tags = Array.from({ length: 60 }, (_, i) => `tag-${i}`);
    const relations = Array.from({ length: 30 }, (_, i) => ({
      relType: "relates_to",
      target: { domain: null, target: `t-${String(i).padStart(2, "0")}` },
      resolved: true,
      targetTitle: null,
      targetSalience: null,
    }));
    const room = generateRoom({ ...CANNED_BRIDGE, tags, relations });
    expect(room.width).toBeLessThanOrEqual(ROOM_CAP);
    expect(room.depth).toBeLessThanOrEqual(ROOM_CAP);
    const keys = room.fixtures.map(slotKey);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("builds a bare room for an engram with nothing in it", () => {
    const room = generateRoom({
      ...CANNED_BRIDGE,
      type: null,
      tags: [],
      content: "",
      relations: [],
      links: [],
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
          targetTitle: null,
          targetSalience: null,
        },
      ],
    });
    const portal = room.fixtures.find((f) => f.kind === "portal");
    expect(portal?.kind === "portal" && portal.sealed).toBe(true);
    expect(portal?.kind === "portal" && portal.label).toBe("?FILE NOT FOUND");
  });
});
