import { afterEach, describe, expect, it, vi } from "vitest";

import {
  AMBIENCE_KEY,
  BUS_LEVELS,
  MASTER_LEVEL,
  MUTE_KEY,
  createMixer,
  type Bus,
} from "./mixer";
import { FakeAudioContext, type FakeGain, type FakeNode } from "./testContext";

const BUSES = Object.keys(BUS_LEVELS) as Bus[];

/** The gain that feeds the context's destination. */
function masterOf(ctx: FakeAudioContext): FakeGain {
  const master = ctx
    .ofKind("gain")
    .find((g) => g.connections.includes(ctx.destination));
  if (master === undefined) throw new Error("nothing feeds the destination");
  return master;
}

/** Lets rejected promises surface before the test ends. */
const settle = (): Promise<void> => new Promise((r) => setTimeout(r, 0));

afterEach(() => {
  vi.restoreAllMocks();
  window.localStorage.clear();
});

describe("createMixer", () => {
  // Mutation caught: a method dereferencing a null context (a throw), or
  // `running` true before the context was resumed.
  it("stays silent and never throws without permission (Review Focus 5)", async () => {
    const none = createMixer();
    expect(none.ctx).toBeNull();
    expect(none.running).toBe(false);
    expect(BUSES).toHaveLength(4);
    for (const bus of BUSES) expect(none.bus(bus)).toBeNull();
    expect(() => {
      none.unlock();
      none.suspend();
      none.resume();
      none.toggleMute();
      none.toggleAmbience();
      none.close();
    }).not.toThrow();

    const ctx = new FakeAudioContext();
    ctx.state = "suspended";
    const borrowed = createMixer({ borrow: () => ctx });
    expect(borrowed.ctx).toBe(ctx);
    expect(borrowed.running).toBe(false);
    borrowed.unlock();
    expect(borrowed.running).toBe(true);
    borrowed.suspend();
    expect(borrowed.running).toBe(false);
    borrowed.resume();
    expect(borrowed.running).toBe(true);
    expect(ctx.calls).toEqual(["resume", "suspend", "resume"]);

    // A context that is already closed refuses a resume; nothing surfaces.
    const dead = new FakeAudioContext();
    dead.state = "closed";
    const late = createMixer({ borrow: () => dead });
    expect(() => {
      late.unlock();
      late.resume();
      late.suspend();
      late.close();
    }).not.toThrow();
    // A factory that throws (no WebAudio at all) leaves the mixer silent.
    const refused = createMixer({
      borrow: () => {
        throw new Error("no audio");
      },
      make: () => {
        throw new Error("no audio");
      },
    });
    expect(() => refused.unlock()).not.toThrow();
    expect(refused.ctx).toBeNull();
    await settle();
  });

  // Mutation caught: a state gate on `suspend` or `resume` (a pause right
  // after an unlock whose resume has not landed would be skipped, and a
  // Safari context "interrupted" by a call would never be woken).
  it("calls through while a state is pending and wakes an interrupted context", () => {
    const ctx = new FakeAudioContext();
    ctx.deferred = true;
    const mixer = createMixer({ borrow: () => ctx });
    mixer.unlock();
    expect(ctx.state).toBe("suspended");
    mixer.suspend();
    expect(ctx.calls).toEqual(["resume", "suspend"]);
    ctx.settle();
    expect(ctx.state).toBe("suspended");
    mixer.unlock();
    expect(ctx.calls).toEqual(["resume", "suspend", "resume"]);

    const called = new FakeAudioContext();
    called.state = "interrupted";
    const woken = createMixer({ borrow: () => called });
    woken.resume();
    expect(called.calls).toEqual(["resume"]);
    expect(woken.running).toBe(true);
    called.state = "interrupted";
    woken.unlock();
    expect(called.calls).toEqual(["resume", "resume"]);
  });

  // Mutation caught: `make` called at creation (outside the user's
  // gesture, where Safari refuses to start a context), or on every unlock.
  it("makes a context on unlock when none was borrowed", () => {
    const ctx = new FakeAudioContext();
    const make = vi.fn(() => ctx);
    const mixer = createMixer({ make });
    expect(make).not.toHaveBeenCalled();
    expect(mixer.bus("effects")).toBeNull();

    mixer.unlock();
    expect(make).toHaveBeenCalledTimes(1);
    expect(mixer.ctx).toBe(ctx);
    expect(mixer.running).toBe(true);
    expect(mixer.bus("effects")).not.toBeNull();

    mixer.unlock();
    expect(make).toHaveBeenCalledTimes(1);
  });

  // Mutation caught: a bus straight to the destination (mute would miss
  // it), or a level off the ruled mix (M4 C21).
  it("routes buses through the master at the ruled levels (M4 C21)", () => {
    const ctx = new FakeAudioContext();
    const mixer = createMixer({ borrow: () => ctx });
    const master = masterOf(ctx);
    expect(master.gain.value).toBe(MASTER_LEVEL);
    expect(MASTER_LEVEL).toBe(0.7);
    expect(BUS_LEVELS).toEqual({
      effects: 1.0,
      signature: 0.9,
      ambience: 0.22,
      modem: 0.45,
    });
    expect(BUSES).toHaveLength(4);
    for (const name of BUSES) {
      const bus = mixer.bus(name) as FakeGain;
      expect(bus.kind).toBe("gain");
      expect(bus.gain.value).toBe(BUS_LEVELS[name]);
      expect(bus.connections).toEqual([master]);
    }
    expect(new Set(BUSES.map((b) => mixer.bus(b))).size).toBe(4);
  });

  // Mutation caught: a hard set of the master (a click), the choice not
  // written or not read back, or a storage error escaping.
  it("mutes with a short ramp and remembers", () => {
    const ctx = new FakeAudioContext();
    ctx.currentTime = 1;
    const mixer = createMixer({ borrow: () => ctx });
    expect(mixer.muted).toBe(false);
    expect(mixer.toggleMute()).toBe(true);
    expect(mixer.muted).toBe(true);
    const events = masterOf(ctx).gain.events;
    expect(events.some((e) => e[0] === "setValueAtTime" && e[1] === 0)).toBe(
      false,
    );
    const ramp = events.at(-1)!;
    expect(ramp[0]).toBe("linearRampToValueAtTime");
    expect(ramp[1]).toBe(0);
    expect(ramp[2]).toBeGreaterThan(1);
    expect(ramp[2]).toBeLessThanOrEqual(1.03 + 1e-9);
    expect(window.localStorage.getItem(MUTE_KEY)).toBe("1");

    const other = new FakeAudioContext();
    const next = createMixer({ borrow: () => other });
    expect(next.muted).toBe(true);
    expect(masterOf(other).gain.value).toBe(0);
    expect(next.toggleMute()).toBe(false);
    expect(window.localStorage.getItem(MUTE_KEY)).toBe("0");
    expect(masterOf(other).gain.events.at(-1)?.[1]).toBe(MASTER_LEVEL);

    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const blocked = createMixer();
    expect(blocked.muted).toBe(false);
    expect(blocked.toggleMute()).toBe(true);
    expect(blocked.toggleMute()).toBe(false);
  });

  // Mutation caught: the master ramped instead of the ambience bus, another
  // bus ramped with it, the choice not written, or the ramp a hard set.
  it("switches the ambience bus alone with a short ramp and remembers", () => {
    const ctx = new FakeAudioContext();
    ctx.currentTime = 2;
    const mixer = createMixer({ borrow: () => ctx });
    const master = masterOf(ctx);
    const masterEvents = master.gain.events.length;
    const others = BUSES.filter((b) => b !== "ambience");
    expect(others).toHaveLength(3);
    const otherEvents = others.map(
      (b) => (mixer.bus(b) as FakeGain).gain.events.length,
    );
    expect(mixer.ambienceOff).toBe(false);
    expect(mixer.toggleAmbience()).toBe(true);
    expect(mixer.ambienceOff).toBe(true);
    expect(mixer.muted).toBe(false);
    const ambience = mixer.bus("ambience") as FakeGain;
    const ramp = ambience.gain.events.at(-1)!;
    expect(ramp[0]).toBe("linearRampToValueAtTime");
    expect(ramp[1]).toBe(0);
    expect(ramp[2]).toBeGreaterThan(2);
    expect(ramp[2]).toBeLessThanOrEqual(2.03 + 1e-9);
    expect(master.gain.events).toHaveLength(masterEvents);
    others.forEach((b, i) =>
      expect((mixer.bus(b) as FakeGain).gain.events).toHaveLength(
        otherEvents[i]!,
      ),
    );
    expect(window.localStorage.getItem(AMBIENCE_KEY)).toBe("1");
    expect(window.localStorage.getItem(MUTE_KEY)).toBeNull();

    expect(mixer.toggleAmbience()).toBe(false);
    expect(ambience.gain.events.at(-1)?.[1]).toBe(BUS_LEVELS.ambience);
    expect(window.localStorage.getItem(AMBIENCE_KEY)).toBe("0");
  });

  // Mutation caught: the bus built at its level while the choice says off
  // (a reload would play the drones), or the other buses built at zero.
  it("builds the ambience bus at 0 from the remembered state", () => {
    window.localStorage.setItem(AMBIENCE_KEY, "1");
    const ctx = new FakeAudioContext();
    const mixer = createMixer({ borrow: () => ctx });
    expect(mixer.ambienceOff).toBe(true);
    expect((mixer.bus("ambience") as FakeGain).gain.value).toBe(0);
    for (const b of BUSES.filter((n) => n !== "ambience"))
      expect((mixer.bus(b) as FakeGain).gain.value).toBe(BUS_LEVELS[b]);
    expect(masterOf(ctx).gain.value).toBe(MASTER_LEVEL);

    const later = new FakeAudioContext();
    const made = createMixer({ make: () => later });
    made.unlock();
    expect((made.bus("ambience") as FakeGain).gain.value).toBe(0);
  });

  // Mutation caught: a storage error escaping the toggle.
  it("toggles the ambience when the storage refuses", () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    const blocked = createMixer();
    expect(blocked.ambienceOff).toBe(false);
    expect(blocked.toggleAmbience()).toBe(true);
    expect(blocked.toggleAmbience()).toBe(false);
  });

  // Mutation caught: the remembered mute applied only to a borrowed
  // context, not to one made later on unlock.
  it("applies a remembered mute to a context made on unlock", () => {
    window.localStorage.setItem(MUTE_KEY, "1");
    const ctx = new FakeAudioContext();
    const mixer = createMixer({ make: () => ctx });
    expect(mixer.muted).toBe(true);
    mixer.unlock();
    expect(masterOf(ctx).gain.value).toBe(0);
  });

  // Mutation caught: the borrowed context closed (the next mount in
  // StrictMode would get a dead one), or an owned one closed twice or
  // left open.
  it("closes what it owns and leaves what it borrowed", async () => {
    const owned = new FakeAudioContext();
    const make = vi.fn(() => owned);
    const made = createMixer({ make });
    made.unlock();
    const master: FakeNode = masterOf(owned);
    made.close();
    made.close();
    expect(owned.calls.filter((c) => c === "close")).toHaveLength(1);
    expect(made.bus("effects")).toBeNull();
    expect(made.ctx).toBeNull();
    expect(made.running).toBe(false);
    expect(master.disconnects).toBeGreaterThan(0);
    // A closed mixer stays closed: no new context on a later gesture.
    made.unlock();
    expect(make).toHaveBeenCalledTimes(1);
    expect(made.ctx).toBeNull();

    const lent = new FakeAudioContext();
    lent.state = "running";
    const borrowed = createMixer({ borrow: () => lent });
    borrowed.close();
    expect(lent.calls).toEqual(["suspend"]);
    expect(lent.state).toBe("suspended");
    expect(borrowed.bus("modem")).toBeNull();
    await settle();
  });
});
