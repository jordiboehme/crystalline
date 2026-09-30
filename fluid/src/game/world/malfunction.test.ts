/**
 * The fault clock: which ways are broken, one seeded run of frames per
 * kind, and the per-tick step.
 *
 * Every case runs on `galleryRoom()`, which carries one broken door
 * (`?FILE NOT FOUND`) and one broken portal (`?FILE NOT FOUND`) of its own,
 * plus a door sealed `NO ROUTE` (never broken, M1) and open doors, hatches
 * and portals a test can fail through `failed`. Fixtures are found by kind
 * ordinal with `nth`, never by their position in the list.
 */

import { describe, expect, it } from "vitest";

import { galleryRoom } from "./canned";
import { ACCESS_DENIED, NOT_FOUND } from "./generate";
import { approaches, wallPoint, type DoorState } from "./interact";
import {
  DOOR_JERK,
  DOOR_PEAK_MAX,
  DOOR_PEAK_MIN,
  DOOR_SLAM,
  LAMP_ON,
  REST_FRAME,
  SHUDDER,
  START_MAX,
  WAIT_MAX,
  WAIT_MIN,
  armFault,
  brokenWays,
  faultFrames,
  faultSeed,
  isBrokenWay,
  newFault,
  planRun,
  stepFaults,
  type Fault,
  type FaultFrame,
} from "./malfunction";
import type { Player } from "./move";
import type { Fixture, RoomSpec } from "./types";

/** The index of the `n`th fixture of `kind`, in fixture order. */
const nth = (room: RoomSpec, kind: Fixture["kind"], n: number) => {
  let seen = -1;
  const i = room.fixtures.findIndex((f) => f.kind === kind && ++seen === n);
  if (i < 0) throw new Error(`no ${kind} ${String(n)}`);
  return i;
};

/** The player 1 m in front of fixture `i`, facing it. */
const facing = (room: RoomSpec, i: number, d = 1): Player => {
  const f = room.fixtures[i];
  if (f === undefined) throw new Error("no fixture");
  const w = wallPoint(f.slot);
  return {
    x: w.x + w.inward[0] * d,
    z: w.z + w.inward[1] * d,
    yaw: Math.atan2(w.inward[0], w.inward[1]),
    vx: 0,
    vz: 0,
    pitch: 0,
    bob: 0,
  };
};

/** A player 8 m in front of fixture `i`: outside `APPROACH`. */
const away = (room: RoomSpec, i: number): Player => facing(room, i, 8);

describe("brokenWays", () => {
  it("counts ?FILE NOT FOUND ways and failed ways, never NO ROUTE or an open way", () => {
    const room = galleryRoom();
    const door4 = nth(room, "door", 4);
    const portal2 = nth(room, "portal", 2);
    expect(brokenWays(room, new Map())).toEqual([door4, portal2]);

    const hatch0 = nth(room, "hatch", 0);
    const door1 = nth(room, "door", 1);
    const failed = new Map([
      [hatch0, NOT_FOUND],
      [door1, NOT_FOUND],
    ]);
    expect(brokenWays(room, failed)).toEqual(
      [door1, door4, portal2, hatch0].sort((a, b) => a - b),
    );

    expect(isBrokenWay(room, nth(room, "door", 3), new Map())).toBe(false);
  });

  it("counts a statically sealed ACCESS_DENIED fixture as broken too", () => {
    // The generator never produces this seal (M2), but the type allows it,
    // and a denied target must count the same as an unresolved one.
    const room = galleryRoom();
    const doorI = nth(room, "door", 1);
    const original = room.fixtures[doorI];
    if (original === undefined || original.kind !== "door") {
      throw new Error("no door");
    }
    const sealed: RoomSpec = {
      ...room,
      fixtures: room.fixtures.map((f, i) =>
        i === doorI
          ? { ...original, address: null, sealedLabel: ACCESS_DENIED }
          : f,
      ),
    };
    expect(isBrokenWay(sealed, doorI, new Map())).toBe(true);
    expect(brokenWays(sealed, new Map())).toContain(doorI);
  });
});

describe("planRun", () => {
  it("is the same for the same fixture and run, and differs from run to run", () => {
    expect(planRun("door", 42, 0)).toEqual(planRun("door", 42, 0));
    expect(planRun("door", 42, 0)).not.toEqual(planRun("door", 42, 1));
  });

  it("jerks a door to 30-50 percent, shudders with sparks and a blinking lamp, holds and slams shut", () => {
    for (let run = 0; run < 20; run++) {
      const frames = planRun("door", 7, run);
      expect(frames.length).toBeGreaterThanOrEqual(
        DOOR_JERK + 10 + 7 + DOOR_SLAM,
      );
      expect(frames.length).toBeLessThanOrEqual(
        DOOR_JERK + 16 + 21 + DOOR_SLAM,
      );
      for (const f of frames) {
        expect(f.open).toBeGreaterThanOrEqual(0);
        expect(f.open).toBeLessThanOrEqual(DOOR_PEAK_MAX + SHUDDER);
      }
      const peakFrame = frames[DOOR_JERK - 1];
      if (peakFrame === undefined) throw new Error("no jerk frame");
      expect(peakFrame.open).toBeGreaterThanOrEqual(DOOR_PEAK_MIN);
      expect(peakFrame.open).toBeLessThanOrEqual(DOOR_PEAK_MAX);
      const last = frames[frames.length - 1];
      if (last === undefined) throw new Error("no last frame");
      expect(last.open).toBe(0);
      expect(frames.some((f) => f.spark > 0)).toBe(true);
      expect(frames.some((f) => f.lamp === LAMP_ON)).toBe(true);
      expect(frames.some((f) => f.lamp === 0)).toBe(true);
      for (const f of frames) {
        expect(f.scale).toBe(REST_FRAME.scale);
        expect(f.gain).toBe(REST_FRAME.gain);
        expect(f.shift).toBe(REST_FRAME.shift);
      }
    }
  });

  it("pops a hatch lid a crack, rattles it and drops it", () => {
    const frames = planRun("hatch", 3, 0);
    expect(frames.length).toBeGreaterThanOrEqual(12);
    expect(frames.length).toBeLessThanOrEqual(18);
    for (const f of frames) {
      expect(f.open).toBeGreaterThanOrEqual(0);
      expect(f.open).toBeLessThanOrEqual(1);
      expect(f.spark).toBe(0);
      expect(f.lamp).toBe(0);
    }
    expect(frames[0]?.open).toBe(0.5);
    expect(frames[1]?.open).toBe(1);
    const last = frames[frames.length - 1];
    expect(last?.open).toBe(0);
  });

  it("stutters a portal, collapses it to a point and restarts it", () => {
    const frames = planRun("portal", 11, 0);
    expect(frames.length).toBeGreaterThanOrEqual(36);
    expect(frames.length).toBeLessThanOrEqual(54);
    expect(frames.some((f) => f.scale === 0)).toBe(true);
    const last = frames[frames.length - 1];
    if (last === undefined) throw new Error("no last frame");
    expect(last.scale).toBe(1);
    expect(last.gain).toBe(1);
    expect(last.shift).toBe(0);
    // The stutter is at least 12 frames (`rng.int(12, 20)`): every one of
    // those ticks moves the shift from the one before it, and some gain in
    // that stretch is off 1.
    const stutter = frames.slice(0, 12);
    for (let t = 1; t < stutter.length; t++) {
      expect(stutter[t]?.shift).not.toBe(stutter[t - 1]?.shift);
    }
    expect(stutter.some((f) => f.gain !== 1)).toBe(true);
    for (const f of frames) expect(f.open).toBe(0);
  });
});

describe("stepFaults", () => {
  it("starts a run within START_MAX ticks of the player coming near, then waits 2 to 5 s", () => {
    const room = galleryRoom();
    const i = nth(room, "door", 4);
    const player = facing(room, i);
    let faults = new Map<number, Fault>();
    let firstStart = -1;
    let runEnd = -1;
    let secondStart = -1;
    for (let t = 1; t <= 300; t++) {
      faults = stepFaults(room, player, faults, new Map(), null, new Map());
      const f = faults.get(i);
      const running = f !== undefined && f.frames !== null;
      expect(faultFrames(faults).has(i)).toBe(running);
      if (firstStart < 0 && running) firstStart = t;
      if (firstStart >= 0 && runEnd < 0 && !running) runEnd = t;
      if (runEnd >= 0 && secondStart < 0 && running) secondStart = t;
    }
    expect(firstStart).toBeGreaterThan(0);
    expect(firstStart).toBeLessThanOrEqual(START_MAX + 1);
    expect(runEnd).toBeGreaterThan(0);
    expect(secondStart).toBeGreaterThan(0);
    const gap = secondStart - runEnd;
    expect(gap).toBeGreaterThanOrEqual(WAIT_MIN);
    expect(gap).toBeLessThanOrEqual(WAIT_MAX + 1);
  });

  it("finishes a run when the player walks away and starts no new one until they return", () => {
    const room = galleryRoom();
    const i = nth(room, "door", 4);
    const near = facing(room, i);
    const far = away(room, i);

    const runToEnd = (start: Map<number, Fault>) => {
      let faults = start;
      let started = false;
      for (let t = 0; t < START_MAX + 1 && !started; t++) {
        faults = stepFaults(room, near, faults, new Map(), null, new Map());
        started = faults.get(i)?.frames !== null;
      }
      expect(started).toBe(true);
      let running = true;
      for (let t = 0; t < 60 && running; t++) {
        faults = stepFaults(room, far, faults, new Map(), null, new Map());
        running = faults.get(i)?.frames !== null;
      }
      expect(running).toBe(false);
      return faults;
    };

    let faults = runToEnd(new Map());
    for (let t = 0; t < WAIT_MAX + 10; t++) {
      faults = stepFaults(room, far, faults, new Map(), null, new Map());
      expect(faults.get(i)?.frames).toBeNull();
    }
    faults = stepFaults(room, near, faults, new Map(), null, new Map());
    expect(faults.get(i)?.frames).not.toBeNull();

    let faults2 = runToEnd(new Map());
    for (let t = 0; t < 5; t++) {
      faults2 = stepFaults(room, far, faults2, new Map(), null, new Map());
    }
    faults2 = stepFaults(room, near, faults2, new Map(), i, new Map());
    expect(faults2.get(i)?.frames).not.toBeNull();
  });

  it("gives two broken doors of one room different timings", () => {
    const room = galleryRoom();
    const d0 = nth(room, "door", 0);
    const d1 = nth(room, "door", 1);
    const failed = new Map([
      [d0, NOT_FOUND],
      [d1, NOT_FOUND],
    ]);
    const f0 = room.fixtures[d0];
    const f1 = room.fixtures[d1];
    if (f0 === undefined || f1 === undefined) throw new Error("no door");
    const w0 = wallPoint(f0.slot);
    const w1 = wallPoint(f1.slot);
    const player: Player = {
      x: (w0.x + w1.x) / 2 + w0.inward[0],
      z: (w0.z + w1.z) / 2 + w0.inward[1],
      yaw: 0,
      vx: 0,
      vz: 0,
      pitch: 0,
      bob: 0,
    };
    expect(approaches(f0.slot, player)).toBe(true);
    expect(approaches(f1.slot, player)).toBe(true);

    let faults = new Map<number, Fault>();
    const opens0: number[] = [];
    const opens1: number[] = [];
    let firstStart0 = -1;
    let firstStart1 = -1;
    let firstFrame0: FaultFrame | undefined;
    let firstFrame1: FaultFrame | undefined;
    for (let t = 1; t <= 600; t++) {
      faults = stepFaults(room, player, faults, failed, null, new Map());
      const frames = faultFrames(faults);
      const fr0 = frames.get(d0);
      const fr1 = frames.get(d1);
      opens0.push(fr0?.open ?? 0);
      opens1.push(fr1?.open ?? 0);
      if (firstStart0 < 0 && fr0 !== undefined) {
        firstStart0 = t;
        firstFrame0 = fr0;
      }
      if (firstStart1 < 0 && fr1 !== undefined) {
        firstStart1 = t;
        firstFrame1 = fr1;
      }
    }
    expect(firstStart0).toBeGreaterThan(0);
    expect(firstStart1).toBeGreaterThan(0);
    expect(opens0).not.toEqual(opens1);
    const sameStart = firstStart0 === firstStart1;
    const sameFrame =
      firstFrame0 !== undefined &&
      firstFrame1 !== undefined &&
      JSON.stringify(firstFrame0) === JSON.stringify(firstFrame1);
    expect(sameStart && sameFrame).toBe(false);
  });

  it("delays the first run only while the player approaches, not from room entry (M11)", () => {
    const room = galleryRoom();
    const d0 = nth(room, "door", 0);
    const d1 = nth(room, "door", 1);
    const failed = new Map([
      [d0, NOT_FOUND],
      [d1, NOT_FOUND],
    ]);
    const f0 = room.fixtures[d0];
    const f1 = room.fixtures[d1];
    if (f0 === undefined || f1 === undefined) throw new Error("no door");
    const w0 = wallPoint(f0.slot);
    const w1 = wallPoint(f1.slot);
    const nearBoth: Player = {
      x: (w0.x + w1.x) / 2 + w0.inward[0],
      z: (w0.z + w1.z) / 2 + w0.inward[1],
      yaw: 0,
      vx: 0,
      vz: 0,
      pitch: 0,
      bob: 0,
    };
    const farAway: Player = {
      ...nearBoth,
      x: nearBoth.x + 100,
      z: nearBoth.z + 100,
    };
    expect(approaches(f0.slot, nearBoth)).toBe(true);
    expect(approaches(f1.slot, nearBoth)).toBe(true);
    expect(approaches(f0.slot, farAway)).toBe(false);
    expect(approaches(f1.slot, farAway)).toBe(false);

    // The player walks for 50 ticks, well past START_MAX, before it ever
    // approaches either door: room entry must not spend the delay.
    let faults = new Map<number, Fault>();
    for (let t = 0; t < 50; t++) {
      faults = stepFaults(room, farAway, faults, failed, null, new Map());
    }

    let firstStart0 = -1;
    let firstStart1 = -1;
    for (let t = 1; t <= START_MAX + 1; t++) {
      faults = stepFaults(room, nearBoth, faults, failed, null, new Map());
      if (firstStart0 < 0 && faults.get(d0)?.frames !== null) firstStart0 = t;
      if (firstStart1 < 0 && faults.get(d1)?.frames !== null) firstStart1 = t;
    }
    expect(firstStart0).toBeGreaterThan(0);
    expect(firstStart1).toBeGreaterThan(0);
    expect(firstStart0).not.toBe(firstStart1);
  });

  it("keeps no fault for a way that is not broken", () => {
    const room = galleryRoom();
    const i = nth(room, "door", 3);
    const player = facing(room, i);
    let faults = new Map<number, Fault>();
    for (let t = 0; t < 200; t++) {
      faults = stepFaults(room, player, faults, new Map(), null, new Map());
    }
    expect(faults.has(i)).toBe(false);
  });

  it("starts an armed door's run on the tick its door has shut", () => {
    const room = galleryRoom();
    const i = nth(room, "door", 1);
    const failed = new Map([[i, NOT_FOUND]]);
    const player = facing(room, i);
    let faults = armFault(room, i, new Map());
    const doorsOpen: Map<number, DoorState> = new Map([
      [i, { open: 0.5, target: 0 }],
    ]);
    faults = stepFaults(room, player, faults, failed, null, doorsOpen);
    expect(faults.get(i)?.frames).toBeNull();
    const doorsShut: Map<number, DoorState> = new Map([
      [i, { open: 0, target: 0 }],
    ]);
    faults = stepFaults(room, player, faults, failed, null, doorsShut);
    expect(faults.get(i)?.frames).not.toBeNull();
    expect(faults.get(i)?.armed).toBe(false);

    const hi = nth(room, "hatch", 0);
    const hFailed = new Map([[hi, NOT_FOUND]]);
    let hFaults = armFault(room, hi, new Map());
    hFaults = stepFaults(
      room,
      facing(room, hi),
      hFaults,
      hFailed,
      null,
      new Map(),
    );
    expect(hFaults.get(hi)?.frames).not.toBeNull();
  });
});

describe("armFault", () => {
  it("leaves a running fault alone and arms only an idle one", () => {
    const room = galleryRoom();
    const i = room.fixtures.findIndex(
      (f) => f.kind === "door" && f.address !== null,
    );
    const fx = room.fixtures[i];
    if (fx === undefined || fx.kind !== "door") throw new Error("no open door");
    const idle = newFault("door", faultSeed(fx.slot, fx.seed));
    const running: Fault = {
      ...idle,
      runs: 1,
      frames: planRun("door", idle.seed, 0),
      at: 2,
    };
    expect(armFault(room, i, new Map([[i, running]])).get(i)).toEqual(running);
    expect(armFault(room, i, new Map([[i, idle]])).get(i)).toEqual({
      ...idle,
      armed: true,
      wait: 0,
    });
    expect(armFault(room, i, new Map()).get(i)).toEqual({
      ...idle,
      armed: true,
      wait: 0,
    });
  });
});
