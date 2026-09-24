/**
 * Using the room: what the player faces, how doors move, where a way leads
 * and where the player comes out.
 *
 * Every case runs on the canned rooms, so the fixtures are the ones the
 * golden test pins: the bridge has a sliding door, a blast door and a portal
 * on its north wall, two terminals on the west wall and a hatch on the
 * south wall; the hub adds bulkheads and the two sealed doors. Fixtures are
 * found by what they are, never by their position in the list.
 */

import { describe, expect, it } from "vitest";

import { BLAST_HALF, BULK_HALF, SLIDE_HALF } from "../render/models/doors";
import { CANNED_BRIDGE, CANNED_HUB } from "./canned";
import { CELL, NOT_FOUND, generateRoom } from "./generate";
import { isFloor } from "./layout";
import {
  APPROACH,
  DOOR_HALF,
  FACING,
  REACH,
  approaches,
  arrivalSpawn,
  focusOf,
  hatchTravel,
  stepDoors,
  travelOf,
  wallPoint,
  type DoorState,
} from "./interact";
import { PLAYER_RADIUS, blockersFor, type Player } from "./move";
import type { Fixture, RoomSpec } from "./types";

const bridge = generateRoom(CANNED_BRIDGE);
const hub = generateRoom(CANNED_HUB);

/** A player standing still at (x, z), looking along `yaw`. */
function at(x: number, z: number, yaw: number): Player {
  return { x, z, vx: 0, vz: 0, yaw, pitch: 0, bob: 0 };
}

/** The yaw that looks along the horizontal direction (dx, dz). */
function yawAlong(dx: number, dz: number): number {
  return Math.atan2(-dx, -dz);
}

/** The index of the first fixture that passes `test`, or a failed test. */
function indexOf(room: RoomSpec, test: (f: Fixture) => boolean): number {
  const index = room.fixtures.findIndex(test);
  expect(index).toBeGreaterThanOrEqual(0);
  return index;
}

/**
 * A player `depth` metres out from fixture `index`'s wall and `side` metres
 * along it, looking at the wall.
 */
function inFront(
  room: RoomSpec,
  index: number,
  depth: number,
  side = 0,
): Player {
  const w = wallPoint(room.fixtures[index]!.slot);
  const x = w.x + w.inward[0] * depth + w.along[0] * side;
  const z = w.z + w.inward[1] * depth + w.along[1] * side;
  return at(x, z, yawAlong(-w.inward[0], -w.inward[1]));
}

/** `ticks` door ticks with the player standing still and nothing pressed. */
function settle(
  room: RoomSpec,
  player: Player,
  doors: Map<number, DoorState>,
  ticks: number,
) {
  let out = doors;
  for (let i = 0; i < ticks; i++) out = stepDoors(room, player, out, null);
  return out;
}

const slidingIndex = indexOf(
  bridge,
  (f) => f.kind === "door" && f.style === "sliding",
);
const blastIndex = indexOf(
  bridge,
  (f) => f.kind === "door" && f.style === "blast",
);
const portalIndex = indexOf(bridge, (f) => f.kind === "portal");
const hatchIndex = indexOf(bridge, (f) => f.kind === "hatch");
const scopeIndex = indexOf(
  bridge,
  (f) => f.kind === "terminal" && f.heading === "Scope",
);

describe("the constants", () => {
  it("are the brief's", () => {
    expect(REACH).toBe(2.2);
    expect(FACING).toBe(Math.PI / 4);
    expect(APPROACH).toBe(3.0);
  });

  it("match the door openings the models cut", () => {
    expect(DOOR_HALF).toEqual({
      sliding: SLIDE_HALF,
      bulkhead: BULK_HALF,
      blast: BLAST_HALF,
    });
  });
});

describe("focusOf", () => {
  it("picks the terminal the player faces within reach", () => {
    const focus = focusOf(bridge, inFront(bridge, scopeIndex, 1.6));
    expect(focus).toEqual({
      kind: "terminal",
      index: scopeIndex,
      prompt: "E READ Scope",
    });
  });

  it("ignores a terminal behind the player", () => {
    const facing = inFront(bridge, scopeIndex, 1.6);
    expect(focusOf(bridge, { ...facing, yaw: facing.yaw + Math.PI })).toBe(
      null,
    );
  });

  it("ignores a terminal beyond reach", () => {
    expect(focusOf(bridge, inFront(bridge, scopeIndex, REACH + 0.3))).toBe(
      null,
    );
  });

  it("offers to crawl through a hatch", () => {
    const focus = focusOf(bridge, inFront(bridge, hatchIndex, 1.2));
    expect(focus).toEqual({
      kind: "hatch",
      index: hatchIndex,
      prompt: "E CRAWL Crew Handbook links_to",
    });
  });

  it("offers to open a blast door and then to close it", () => {
    const player = inFront(bridge, blastIndex, 1.5);
    expect(focusOf(bridge, player)?.prompt).toBe(
      "E OPEN depends_on Reactor Core",
    );
    const open = new Map([[blastIndex, { open: 1, target: 1 as const }]]);
    expect(focusOf(bridge, player, open)?.prompt).toBe(
      "E CLOSE depends_on Reactor Core",
    );
  });
});

describe("stepDoors", () => {
  it("opens a sliding door on approach and closes it on leaving", () => {
    const near = inFront(bridge, slidingIndex, APPROACH - 0.5);
    let doors = stepDoors(bridge, near, new Map(), null);
    expect(doors.get(slidingIndex)).toEqual({ open: 1 / 12, target: 1 });
    doors = settle(bridge, near, doors, 12);
    expect(doors.get(slidingIndex)).toEqual({ open: 1, target: 1 });
    const away = inFront(bridge, slidingIndex, APPROACH + 1);
    doors = settle(bridge, away, doors, 1);
    expect(doors.get(slidingIndex)?.target).toBe(0);
    doors = settle(bridge, away, doors, 12);
    expect(doors.get(slidingIndex)).toEqual({ open: 0, target: 0 });
  });

  it("leaves a blast door shut when the player only walks up to it", () => {
    const doors = settle(bridge, inFront(bridge, blastIndex, 1), new Map(), 20);
    expect(doors.get(blastIndex)).toEqual({ open: 0, target: 0 });
  });

  it("toggles a bulkhead on a press", () => {
    const bulkhead = indexOf(
      hub,
      (f) => f.kind === "door" && f.style === "bulkhead" && f.address !== null,
    );
    const player = inFront(hub, bulkhead, 1.5);
    let doors = stepDoors(hub, player, new Map(), bulkhead);
    expect(doors.get(bulkhead)?.target).toBe(1);
    doors = settle(hub, player, doors, 12);
    expect(doors.get(bulkhead)?.open).toBe(1);
    doors = stepDoors(hub, player, doors, bulkhead);
    expect(doors.get(bulkhead)?.target).toBe(0);
    doors = settle(hub, player, doors, 12);
    expect(doors.get(bulkhead)).toEqual({ open: 0, target: 0 });
  });

  it("never opens a sealed door, and says why it is sealed", () => {
    const sealed = indexOf(
      hub,
      (f) => f.kind === "door" && f.sealedLabel === NOT_FOUND,
    );
    const player = inFront(hub, sealed, 1.5);
    expect(focusOf(hub, player)).toEqual({
      kind: "door",
      index: sealed,
      prompt: `SEALED ${NOT_FOUND}`,
    });
    let doors = new Map<number, DoorState>();
    for (let i = 0; i < 20; i++) doors = stepDoors(hub, player, doors, sealed);
    expect(doors.get(sealed)).toEqual({ open: 0, target: 0 });
  });
});

describe("travelOf", () => {
  const open = new Map([[slidingIndex, { open: 1, target: 1 as const }]]);

  it("goes through an open door inside its width, close to the wall", () => {
    expect(travelOf(bridge, inFront(bridge, slidingIndex, 0.4), open)).toEqual({
      via: "door",
      fixture: slidingIndex,
      address: { domain: "station", permalink: "old-bridge" },
    });
  });

  it("does not go beside the opening, too far out, or through a closing door", () => {
    expect(
      travelOf(
        bridge,
        inFront(bridge, slidingIndex, 0.4, DOOR_HALF.sliding + 0.1),
        open,
      ),
    ).toBe(null);
    expect(travelOf(bridge, inFront(bridge, slidingIndex, 0.7), open)).toBe(
      null,
    );
    const ajar = new Map([[slidingIndex, { open: 0.85, target: 1 as const }]]);
    expect(travelOf(bridge, inFront(bridge, slidingIndex, 0.4), ajar)).toBe(
      null,
    );
    expect(
      travelOf(bridge, inFront(bridge, slidingIndex, 0.4), new Map()),
    ).toBe(null);
  });

  it("goes through a portal on contact", () => {
    expect(
      travelOf(bridge, inFront(bridge, portalIndex, 0.4), new Map()),
    ).toEqual({
      via: "portal",
      fixture: portalIndex,
      address: { domain: "logistics", permalink: "cargo-manifest" },
    });
    expect(travelOf(bridge, inFront(bridge, portalIndex, 0.6), new Map())).toBe(
      null,
    );
  });

  it("never goes through a sealed portal", () => {
    const sealed = indexOf(
      hub,
      (f) => f.kind === "portal" && f.address === null,
    );
    expect(travelOf(hub, inFront(hub, sealed, 0.4), new Map())).toBe(null);
  });

  it("crawls through a hatch only by its index", () => {
    expect(hatchTravel(bridge, hatchIndex)).toEqual({
      via: "hatch",
      fixture: hatchIndex,
      address: { domain: "station", permalink: "crew-handbook" },
    });
    expect(hatchTravel(bridge, slidingIndex)).toBe(null);
  });
});

describe("arrivalSpawn", () => {
  const entrance = {
    x: (bridge.spawn.x + 0.5) * CELL,
    z: (bridge.spawn.y + 0.5) * CELL,
    yaw: bridge.spawn.yaw,
  };

  /** Where 1.6 m in front of fixture `index` is, facing away from its wall. */
  function before(index: number) {
    const w = wallPoint(bridge.fixtures[index]!.slot);
    return {
      x: w.x + w.inward[0] * 1.6,
      z: w.z + w.inward[1] * 1.6,
      yaw: yawAlong(w.inward[0], w.inward[1]),
    };
  }

  function expectAt(
    got: { x: number; z: number; yaw: number },
    want: { x: number; z: number; yaw: number },
  ) {
    expect(got.x).toBeCloseTo(want.x);
    expect(got.z).toBeCloseTo(want.z);
    expect(Math.cos(got.yaw)).toBeCloseTo(Math.cos(want.yaw));
    expect(Math.sin(got.yaw)).toBeCloseTo(Math.sin(want.yaw));
  }

  const handbook = { domain: "station", permalink: "crew-handbook" };

  it("puts a player who came through a door in front of the hatch back", () => {
    expectAt(
      arrivalSpawn(bridge, { via: "door", from: handbook }),
      before(hatchIndex),
    );
    // The hatch is on the south wall: the player faces north, into the room.
    expect(
      Math.cos(arrivalSpawn(bridge, { via: "door", from: handbook }).yaw),
    ).toBeCloseTo(1);
  });

  it("puts a player who came through a portal in front of the hatch back", () => {
    expectAt(
      arrivalSpawn(bridge, { via: "portal", from: handbook }),
      before(hatchIndex),
    );
  });

  it("puts a player who crawled back in front of the door that leads on", () => {
    expectAt(
      arrivalSpawn(bridge, {
        via: "hatch",
        from: { domain: "station", permalink: "old-bridge" },
      }),
      before(slidingIndex),
    );
    expectAt(
      arrivalSpawn(bridge, {
        via: "hatch",
        from: { domain: "logistics", permalink: "cargo-manifest" },
      }),
      before(portalIndex),
    );
  });

  it("falls back to the entrance when nothing matches", () => {
    expectAt(
      arrivalSpawn(bridge, {
        via: "door",
        from: { domain: "station", permalink: "nowhere" },
      }),
      entrance,
    );
    // A hatch never leads back to a hatch.
    expectAt(arrivalSpawn(bridge, { via: "hatch", from: handbook }), entrance);
    expectAt(arrivalSpawn(bridge, null), entrance);
  });
});

/**
 * A point `metres` behind fixture `index`'s wall, on its axis: the far side
 * of the wall, where a bay or the corridor may be.
 */
function behind(room: RoomSpec, index: number, metres: number): Player {
  const w = wallPoint(room.fixtures[index]!.slot);
  return at(
    w.x - w.inward[0] * metres,
    w.z - w.inward[1] * metres,
    yawAlong(w.inward[0], w.inward[1]),
  );
}

describe("behind the wall", () => {
  it("never carries the player through a portal from behind its wall", () => {
    expect(travelOf(bridge, behind(bridge, portalIndex, 3), new Map())).toBe(
      null,
    );
    expect(travelOf(bridge, behind(bridge, portalIndex, 0.2), new Map())).toBe(
      null,
    );
  });

  it("never carries the player through an open door from behind its wall", () => {
    const open = new Map([[slidingIndex, { open: 1, target: 1 as const }]]);
    expect(travelOf(bridge, behind(bridge, slidingIndex, 2), open)).toBe(null);
  });

  it("does not open a sliding door for a player in the bay behind its wall", () => {
    // Every located sliding door of the hub whose wall has floor behind it,
    // one void cell further on: the player stands there against the void,
    // closer to the door's wall point than APPROACH.
    const depth = CELL + PLAYER_RADIUS;
    const cases = hub.fixtures.flatMap((f, index) => {
      if (f.kind !== "door" || f.style !== "sliding" || f.address === null) {
        return [];
      }
      const p = behind(hub, index, depth);
      const floor = isFloor(
        hub.grid,
        Math.floor(p.x / CELL),
        Math.floor(p.z / CELL),
      );
      return floor ? [{ index, p }] : [];
    });
    expect(cases.length).toBeGreaterThan(0);
    expect(depth).toBeLessThan(APPROACH);
    for (const { index, p } of cases) {
      const doors = settle(hub, p, new Map(), 12);
      expect(doors.get(index)).toEqual({ open: 0, target: 0 });
    }
  });

  it("offers nothing behind the wall", () => {
    expect(focusOf(bridge, behind(bridge, scopeIndex, 1))).toBe(null);
  });

  it("says a way is approached only from the front", () => {
    const w = bridge.fixtures[slidingIndex]!.slot;
    expect(approaches(w, inFront(bridge, slidingIndex, 2))).toBe(true);
    expect(approaches(w, inFront(bridge, slidingIndex, APPROACH + 0.1))).toBe(
      false,
    );
    expect(approaches(w, behind(bridge, slidingIndex, 1))).toBe(false);
  });
});

describe("arrivals land on clear floor", () => {
  for (const [name, room] of [
    ["bridge", bridge],
    ["hub", hub],
  ] as const) {
    it(`in the ${name}, in front of every hatch, door and portal`, () => {
      const blockers = blockersFor(room);
      let checked = 0;
      for (const f of room.fixtures) {
        if (f.kind !== "hatch" && f.kind !== "door" && f.kind !== "portal") {
          continue;
        }
        if (f.address === null) continue;
        const via = f.kind === "hatch" ? "door" : "hatch";
        const spot = arrivalSpawn(room, { via, from: f.address });
        expect(
          isFloor(
            room.grid,
            Math.floor(spot.x / CELL),
            Math.floor(spot.z / CELL),
          ),
        ).toBe(true);
        for (const b of blockers) {
          const inside =
            spot.x > b.x0 - PLAYER_RADIUS &&
            spot.x < b.x1 + PLAYER_RADIUS &&
            spot.z > b.z0 - PLAYER_RADIUS &&
            spot.z < b.z1 + PLAYER_RADIUS;
          expect(inside, `${f.kind} ${f.label} lands in a blocker`).toBe(false);
        }
        checked++;
      }
      expect(checked).toBeGreaterThan(0);
    });
  }
});
