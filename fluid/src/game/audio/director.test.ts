import { afterEach, describe, expect, it } from "vitest";

import { CROSSFADE_S } from "./ambience";
import { RIDE_FADE_S, VOICE_CAP, createDirector } from "./director";
import { createMixer, type Mixer } from "./mixer";
import { midiHz } from "./patch";
import {
  FakeAudioContext,
  type FakeGain,
  type FakeNode,
  type FakePanner,
} from "./testContext";

/** A running context and a mixer borrowing it. */
function setup(state: AudioContextState = "running") {
  const ctx = new FakeAudioContext();
  ctx.state = state;
  const mixer = createMixer({ borrow: () => ctx });
  const director = createDirector(mixer);
  return { ctx, mixer, director };
}

/** The bus gain of `name`, as the fake made it. */
function bus(mixer: Mixer, name: Parameters<Mixer["bus"]>[0]): FakeNode {
  const node = mixer.bus(name);
  if (node === null) throw new Error(`no ${name} bus`);
  return node as FakeNode;
}

/** The patch levels feeding `node` directly: one per patch played into it. */
function levelsInto(ctx: FakeAudioContext, node: FakeNode): FakeGain[] {
  return ctx.ofKind("gain").filter((g) => g.connections.includes(node));
}

/** Whether a patch level was stopped (its stop cancels and ramps it). */
const stopped = (level: FakeGain) =>
  level.gain.events.some(([m]) => m === "cancelScheduledValues");

/** The ramp to silence a stop scheduled on a level, as [value, time]. */
function fadeOf(level: FakeGain): [number, number] | undefined {
  const ramp = level.gain.events.find(
    ([m, v]) => m === "linearRampToValueAtTime" && v === 0,
  );
  return ramp === undefined ? undefined : [ramp[1], ramp[2]];
}

/** Every oscillator frequency the context was told to start at. */
const pitches = (ctx: FakeAudioContext) =>
  ctx.ofKind("oscillator").map((o) => o.frequency.events[0]?.[1]);

/** Lets queued microtasks run. */
const microtasks = () => new Promise<void>((r) => queueMicrotask(r));

let disposers: (() => void)[] = [];
afterEach(() => {
  for (const d of disposers) d();
  disposers = [];
  window.localStorage.clear();
});

describe("createDirector", () => {
  // Mutation caught: the pan dropped (no panner, or one left at 0), a door
  // played on the wrong bus, or the distance gain ignored.
  it("plays a door on the effects bus through a panner at its pan", () => {
    const { ctx, mixer, director } = setup();
    disposers.push(() => director.dispose());
    director.cue({
      kind: "door",
      sound: "sliding",
      open: true,
      pan: -0.6,
      gain: 0.5,
    });
    const panners = ctx.ofKind("panner") as FakePanner[];
    expect(panners).toHaveLength(1);
    const panner = panners[0]!;
    expect(panner.pan.value).toBeCloseTo(-0.6, 9);
    expect(panner.connections).toEqual([bus(mixer, "effects")]);
    expect(levelsInto(ctx, panner)).toHaveLength(1);
    // The hiss's envelope peaks at its gain (0.5) times the cue's (0.5).
    const peaks = ctx
      .ofKind("gain")
      .flatMap((g) =>
        g.gain.events
          .filter(([m]) => m === "linearRampToValueAtTime")
          .map(([, v]) => v),
      );
    expect(Math.max(...peaks)).toBeCloseTo(0.25, 9);
  });

  // Mutation caught: the drone restarted on every room of the same kind.
  it("keeps one drone through rooms of the same ambience", () => {
    const { ctx, mixer, director } = setup();
    disposers.push(() => director.dispose());
    director.cue({ kind: "room", ambience: "clean", seed: 1 });
    director.cue({ kind: "room", ambience: "clean", seed: 2 });
    const drones = levelsInto(ctx, bus(mixer, "ambience"));
    expect(drones).toHaveLength(1);
    expect(stopped(drones[0]!)).toBe(false);
  });

  // Mutation caught: no cross-fade (the old drone cut in a click), or the
  // new drone never started.
  it("cross-fades to another ambience over CROSSFADE_S", () => {
    const { ctx, mixer, director } = setup();
    disposers.push(() => director.dispose());
    director.cue({ kind: "room", ambience: "clean", seed: 1 });
    ctx.currentTime = 5;
    director.cue({ kind: "room", ambience: "hangar", seed: 1 });
    const [old, fresh] = levelsInto(ctx, bus(mixer, "ambience"));
    expect(fadeOf(old!)).toEqual([0, 5 + CROSSFADE_S]);
    expect(fresh).toBeDefined();
    expect(stopped(fresh!)).toBe(false);
    // The new drone's voices fade in over the same time.
    expect(pitches(ctx)).toContain(33);
  });

  // Mutation caught: the cap not enforced, the drone or a door evicted
  // before the steps.
  it("holds at most VOICE_CAP live patches, dropping steps first", () => {
    const { ctx, mixer, director } = setup();
    disposers.push(() => director.dispose());
    director.cue({ kind: "room", ambience: "clean", seed: 1 });
    director.cue({
      kind: "door",
      sound: "bulkhead",
      open: true,
      pan: 0,
      gain: 1,
    });
    for (let n = 0; n < 30; n++) {
      director.cue({ kind: "step", foot: (n % 2) as 0 | 1, run: false, n });
    }
    const drone = levelsInto(ctx, bus(mixer, "ambience"));
    const door = levelsInto(ctx, ctx.ofKind("panner")[0]!);
    const steps = levelsInto(ctx, bus(mixer, "effects"));
    expect(steps).toHaveLength(30);
    const live = [...drone, ...door, ...steps].filter((l) => !stopped(l));
    expect(live.length).toBeLessThanOrEqual(VOICE_CAP);
    expect(stopped(drone[0]!)).toBe(false);
    expect(stopped(door[0]!)).toBe(false);
    // The oldest steps went, the newest play on.
    expect(stopped(steps[0]!)).toBe(true);
    expect(stopped(steps.at(-1)!)).toBe(false);
  });

  // Mutation caught: the ride's hum never stopped, or stopped with a
  // click, or the chime missing on a landing.
  it("holds the ride's hum until it arrives, then chimes as the room lands", async () => {
    const { ctx, mixer, director } = setup();
    disposers.push(() => director.dispose());
    director.cue({ kind: "ride", phase: "depart" });
    const effects = bus(mixer, "effects");
    const before = levelsInto(ctx, effects);
    // The doors' hiss and the hum.
    expect(before).toHaveLength(2);
    const hum = before[1]!;
    expect(stopped(hum)).toBe(false);
    ctx.currentTime = 2;
    director.cue({ kind: "ride", phase: "arrive" });
    expect(fadeOf(hum)).toEqual([0, 2 + RIDE_FADE_S]);
    director.cue({ kind: "room", ambience: "clean", seed: 1 });
    expect(pitches(ctx)).toContain(midiHz(84));
    await microtasks();
  });

  // Mutation caught: a chime on a ride that ended without a room (a
  // failed stop, a load error), or a hum left humming after it.
  it("stops the hum without a chime when no room follows the ride", async () => {
    const { ctx, mixer, director } = setup();
    disposers.push(() => director.dispose());
    director.cue({ kind: "ride", phase: "depart" });
    director.cue({ kind: "ride", phase: "arrive" });
    await microtasks();
    director.cue({ kind: "room", ambience: "clean", seed: 1 });
    expect(pitches(ctx)).not.toContain(midiHz(84));
    const hum = levelsInto(ctx, bus(mixer, "effects"))[1]!;
    expect(stopped(hum)).toBe(true);
  });

  // Mutation caught: no unlock on a gesture while the context is not
  // running (Safari's refused resume after a hidden tab, F29), or one on
  // every gesture.
  it("unlocks again on a click or key while the context is not running", () => {
    const { ctx, director } = setup("suspended");
    disposers.push(() => director.dispose());
    window.dispatchEvent(new MouseEvent("click"));
    expect(ctx.calls).toEqual(["resume"]);
    expect(ctx.state).toBe("running");
    window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyW" }));
    expect(ctx.calls).toEqual(["resume"]);
    ctx.state = "interrupted";
    window.dispatchEvent(new KeyboardEvent("keydown", { code: "KeyW" }));
    expect(ctx.calls).toEqual(["resume", "resume"]);
  });

  // Mutation caught: a room entered before the context existed left
  // silent after the first click made one.
  it("starts the room's drone once a click makes the context", () => {
    const ctx = new FakeAudioContext();
    const mixer = createMixer({ make: () => ctx });
    const director = createDirector(mixer);
    disposers.push(() => director.dispose());
    director.cue({ kind: "room", ambience: "dim", seed: 4 });
    director.cue({ kind: "step", foot: 0, run: false, n: 0 });
    expect(ctx.nodes).toHaveLength(0);
    window.dispatchEvent(new MouseEvent("click"));
    expect(levelsInto(ctx, bus(mixer, "ambience"))).toHaveLength(1);
    expect(pitches(ctx)).toContain(46);
  });

  // Mutation caught: one-shots queued on a frozen clock while the context
  // is suspended (they would all fire at once on the resume).
  it("drops one-shots while the context is not running, not the drone", () => {
    const { ctx, mixer, director } = setup("suspended");
    disposers.push(() => director.dispose());
    director.cue({ kind: "step", foot: 0, run: false, n: 0 });
    director.cue({ kind: "terminal" });
    expect(levelsInto(ctx, bus(mixer, "effects"))).toHaveLength(0);
    director.cue({ kind: "room", ambience: "clean", seed: 1 });
    expect(levelsInto(ctx, bus(mixer, "ambience"))).toHaveLength(1);
  });

  // Mutation caught: mute silencing the cues rather than the master (the
  // drone missing when sound comes back mid-room).
  it("mutes the master only, the cues play on into it", () => {
    const { ctx, mixer, director } = setup();
    disposers.push(() => director.dispose());
    expect(director.toggleMute()).toBe(true);
    expect(mixer.muted).toBe(true);
    director.cue({ kind: "room", ambience: "clean", seed: 1 });
    director.cue({ kind: "terminal" });
    expect(levelsInto(ctx, bus(mixer, "ambience"))).toHaveLength(1);
    expect(levelsInto(ctx, bus(mixer, "effects"))).toHaveLength(1);
    expect(director.toggleMute()).toBe(false);
  });

  // Mutation caught: a drone or a hum left playing after the route went,
  // the mixer left open, or cues still played after.
  it("stops everything and closes the mixer on dispose", () => {
    const { ctx, mixer, director } = setup();
    director.cue({ kind: "room", ambience: "clean", seed: 1 });
    director.cue({ kind: "ride", phase: "depart" });
    director.cue({ kind: "step", foot: 0, run: false, n: 0 });
    const levels = [
      ...levelsInto(ctx, bus(mixer, "ambience")),
      ...levelsInto(ctx, bus(mixer, "effects")),
    ];
    expect(levels).toHaveLength(4);
    director.dispose();
    for (const level of levels) expect(stopped(level)).toBe(true);
    expect(mixer.ctx).toBeNull();
    expect(ctx.calls).toContain("suspend");
    const made = ctx.nodes.length;
    director.cue({ kind: "terminal" });
    window.dispatchEvent(new MouseEvent("click"));
    expect(ctx.nodes).toHaveLength(made);
    expect(ctx.calls).not.toContain("resume");
  });

  // Mutation caught: a click on the pause screen (or while the tab is
  // hidden) unlocking a context the host suspended on purpose.
  it("leaves a suspended context quiet through gestures until resumed", () => {
    const { ctx, director } = setup();
    disposers.push(() => director.dispose());
    director.suspend();
    window.dispatchEvent(new MouseEvent("click"));
    window.dispatchEvent(new KeyboardEvent("keydown", { code: "Enter" }));
    expect(ctx.calls).toEqual(["suspend"]);
    director.resume();
    expect(ctx.calls).toEqual(["suspend", "resume"]);
    // A resume the browser refused: the next gesture unlocks.
    ctx.state = "suspended";
    window.dispatchEvent(new MouseEvent("click"));
    expect(ctx.calls).toEqual(["suspend", "resume", "resume"]);
  });

  // Mutation caught: suspend and resume not passed to the mixer.
  it("suspends and resumes the context", () => {
    const { ctx, director } = setup();
    disposers.push(() => director.dispose());
    director.suspend();
    expect(ctx.state).toBe("suspended");
    director.resume();
    expect(ctx.state).toBe("running");
  });
});
