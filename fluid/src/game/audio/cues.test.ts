/**
 * The cues' pure layer: where a placed cue sits in the stereo field, how
 * many footsteps a stride phase makes, and which door and fault changes
 * make a sound (M4 C22, C23, F15).
 */

import { describe, expect, it } from "vitest";

import { boxKey } from "../world/box";
import { airlockRoom } from "../world/airlock";
import {
  CANNED_DECK,
  CANNED_DOMAINS,
  CANNED_HANGAR,
  galleryRoom,
  heroHallRoom,
} from "../world/canned";
import { consoleRoom } from "../world/consoleRoom";
import { generateDeck } from "../world/deck";
import { doorKey } from "../world/interact";
import type { Fault } from "../world/malfunction";
import { spawnPlayer, stepPlayer, type Player } from "../world/move";
import type { RoomSpec } from "../world/types";
import {
  ambienceOf,
  doorCues,
  faultCues,
  placeCue,
  stepsBetween,
} from "./cues";

const gallery = galleryRoom();

/** The index of the first fixture of `kind` that matches `pick`. */
function fixtureIndex(
  room: RoomSpec,
  pick: (f: RoomSpec["fixtures"][number]) => boolean,
): number {
  const i = room.fixtures.findIndex(pick);
  if (i < 0) throw new Error("no such fixture");
  return i;
}

describe("placeCue", () => {
  it("pans toward the side the player strafes to", () => {
    // Mutation caught: the sign flipped, degrees for radians.
    const at = spawnPlayer(gallery);
    const player: Player = { ...at, yaw: 1.1 };
    const moved = stepPlayer(
      player,
      { forward: 0, strafe: 1, turn: 0, lookDx: 0, lookDy: 0, run: false },
      gallery,
      [],
    );
    const dx = moved.x - player.x;
    const dz = moved.z - player.z;
    const len = Math.hypot(dx, dz);
    expect(len).toBeGreaterThan(0);
    const rx = dx / len;
    const rz = dz / len;

    const right = placeCue(player, player.x + rx, player.z + rz);
    const left = placeCue(player, player.x - rx, player.z - rz);
    // Ahead is a quarter turn from the right, towards the side the
    // strafe is not: a quarter turn of (rx, rz) either way sits at pan 0.
    const ahead = placeCue(player, player.x + rz, player.z - rx);
    expect(right.pan).toBeGreaterThan(0.9);
    expect(left.pan).toBeLessThan(-0.9);
    expect(Math.abs(ahead.pan)).toBeLessThan(0.05);

    const here = placeCue(player, player.x, player.z);
    expect(here.gain).toBe(1);
    expect(here.pan).toBe(0);
    const near = placeCue(player, player.x + rx * 2, player.z + rz * 2);
    const far = placeCue(player, player.x + rx * 20, player.z + rz * 20);
    const beyond = placeCue(player, player.x + rx * 1e6, player.z + rz * 1e6);
    expect(near.gain).toBeLessThan(1);
    expect(far.gain).toBeLessThan(near.gain);
    expect(beyond.gain).toBeGreaterThanOrEqual(0);
  });
});

describe("stepsBetween", () => {
  it("counts one step per half cycle", () => {
    // Mutation caught: one per full cycle.
    expect(stepsBetween(0, Math.PI - 0.01)).toBe(0);
    expect(stepsBetween(0, Math.PI + 0.01)).toBe(1);
    expect(stepsBetween(3, 3 + 2 * Math.PI)).toBe(2);
    expect(stepsBetween(1, 1)).toBe(0);
  });
});

describe("doorCues", () => {
  it("cues a door once per change of its target", () => {
    // Mutation caught: a cue every tick while open.
    const player = spawnPlayer(gallery);
    const sliding = fixtureIndex(
      gallery,
      (f) => f.kind === "door" && f.style === "sliding",
    );
    const shut = new Map([[doorKey(sliding), 0]]);
    const open = new Map([[doorKey(sliding), 1]]);
    expect(doorCues(gallery, player, shut, shut)).toEqual([]);
    expect(doorCues(gallery, player, open, open)).toEqual([]);

    const opened = doorCues(gallery, player, shut, open);
    expect(opened).toHaveLength(1);
    expect(opened[0]).toMatchObject({
      kind: "door",
      sound: "sliding",
      open: true,
    });
    const closed = doorCues(gallery, player, open, shut);
    expect(closed).toEqual([{ ...opened[0], open: false }]);
    // A door absent from a map is shut.
    expect(doorCues(gallery, player, new Map(), open)).toEqual(opened);

    const hall = heroHallRoom();
    const box = hall.heroes.findIndex((h) => h.kind === "police-box");
    expect(box).toBeGreaterThanOrEqual(0);
    const boxCues = doorCues(
      hall,
      spawnPlayer(hall),
      new Map(),
      new Map([[boxKey(box), 1]]),
    );
    expect(boxCues).toHaveLength(1);
    expect(boxCues[0]).toMatchObject({
      kind: "door",
      sound: "box",
      open: true,
    });
  });
});

describe("faultCues", () => {
  it("cues a fault when a run starts", () => {
    // Mutation caught: a cue every frame of the run, the run count not
    // carried.
    const hatch = fixtureIndex(gallery, (f) => f.kind === "hatch");
    const player = spawnPlayer(gallery);
    const fault = (runs: number, at: number): Fault => ({
      kind: "hatch",
      seed: 7,
      runs,
      frames: [],
      at,
      wait: 0,
      armed: false,
    });
    const before = new Map([[hatch, fault(2, 5)]]);
    const running = new Map([[hatch, fault(2, 6)]]);
    expect(faultCues(gallery, player, before, running)).toEqual([]);
    const started = faultCues(
      gallery,
      player,
      before,
      new Map([[hatch, fault(3, 0)]]),
    );
    expect(started).toHaveLength(1);
    expect(started[0]).toMatchObject({ kind: "fault", way: "hatch", run: 3 });
    // A way's first run, with no fault before it.
    const first = faultCues(
      gallery,
      player,
      new Map(),
      new Map([[hatch, fault(1, 0)]]),
    );
    expect(first[0]).toMatchObject({ kind: "fault", run: 1 });
  });
});

describe("ambienceOf", () => {
  it("names the ambience of every room kind (M4 C22)", () => {
    // Mutation caught: the airlock read as console.
    for (const condition of [
      "clean",
      "construction",
      "dim",
      "derelict",
    ] as const) {
      expect(ambienceOf({ ...gallery, condition }, false)).toBe(condition);
    }
    const airlock = airlockRoom({ domains: CANNED_DOMAINS, here: null });
    expect(airlock.interior).toBeDefined();
    expect(ambienceOf(airlock, false)).toBe("airlock");
    const hangar = generateDeck(CANNED_HANGAR, 0);
    expect(hangar.space).toBe("hangar");
    expect(ambienceOf(hangar, false)).toBe("hangar");
    const deck = generateDeck(CANNED_DECK, 0);
    expect(deck.space).toBe("deck");
    expect(ambienceOf(deck, false)).toBe(deck.condition);
    expect(ambienceOf({ ...deck, condition: "dim" }, false)).toBe("dim");
    expect(ambienceOf(consoleRoom(), false)).toBe("console");
    expect(ambienceOf(gallery, true)).toBe("dark");
    expect(ambienceOf(deck, true)).toBe("dark");
  });
});
