import { describe, expect, it } from "vitest";

import type { Patch, Voice } from "./patch";
import { noiseBuffer, playPatch } from "./synth";
import {
  FakeAudioContext,
  FakeDelay,
  type FakeNode,
  type FakeParam,
  type FakeWave,
  type ParamEvent,
} from "./testContext";

/** A voice with every field set, for tests that change one or two. */
function voice(over: Partial<Voice> = {}): Voice {
  return {
    wave: "saw",
    pitch: [{ at: 0, value: 220 }],
    env: { a: 0.01, d: 0.1, s: 0.5, r: 0.2 },
    length: 0.5,
    at: 0,
    gain: 1,
    ...over,
  };
}

/** A one-voice patch. */
function single(v: Voice, loop = false): Patch {
  return loop
    ? { name: "test", voices: [v], loop }
    : { name: "test", voices: [v] };
}

/** The sources (oscillators and buffer players) the context made. */
function sources(ctx: FakeAudioContext): FakeNode[] {
  return ctx.nodes.filter(
    (n) => n.kind === "oscillator" || n.kind === "bufferSource",
  );
}

/** Asserts `param` got the `[method, value, time]` events given, in order. */
function expectEvents(param: FakeParam, want: ParamEvent[]): void {
  expect(param.events).toHaveLength(want.length);
  want.forEach(([method, value, time], i) => {
    const got = param.events[i] as ParamEvent;
    expect(got[0]).toBe(method);
    expect(got[1]).toBeCloseTo(value, 9);
    expect(got[2]).toBeCloseTo(time, 9);
  });
}

/** The gain node that feeds `dest` directly: the patch's own level. */
function patchGain(ctx: FakeAudioContext, dest: FakeNode): FakeNode {
  const found = ctx.ofKind("gain").find((g) => g.connections.includes(dest));
  if (found === undefined) throw new Error("no gain feeds the destination");
  return found;
}

describe("playPatch", () => {
  // Mutation caught: a plain square wave (one oscillator, no delay), or a
  // delay that does not follow the width steps.
  it("builds a width-modulated pulse from two saws and a delay", () => {
    const ctx = new FakeAudioContext();
    const when = 1;
    playPatch(
      ctx,
      ctx.destination,
      single(
        voice({
          wave: "pulse",
          pitch: [{ at: 0, value: 100 }],
          width: [
            { at: 0, value: 0.5 },
            { at: 1, value: 0.1, ramp: "linear" },
          ],
        }),
      ),
      when,
      "pulse",
    );

    const saws = ctx.ofKind("oscillator");
    expect(saws).toHaveLength(2);
    for (const saw of saws) {
      expect(saw.type).toBe("sawtooth");
      expectEvents(saw.frequency, [["setValueAtTime", 100, when]]);
    }
    const inverters = ctx.ofKind("gain").filter((g) => g.gain.value === -1);
    expect(inverters).toHaveLength(1);
    const inverter = inverters[0] as FakeNode;
    // One saw feeds the inverter, the other does not.
    expect(saws.filter((s) => s.connections.includes(inverter))).toHaveLength(
      1,
    );
    const delay = inverter.connections[0];
    expect(delay).toBeInstanceOf(FakeDelay);
    expectEvents((delay as FakeDelay).delayTime, [
      ["setValueAtTime", 0.005, when],
      ["linearRampToValueAtTime", 0.001, when + 1],
    ]);
  });

  // Mutation caught: the delay reading the width only (a pitch step would
  // leave it at 0.005 and the pulse would change width as it changes
  // pitch), or a delay line made too short for the lowest pitch.
  it("keeps the width when the pitch moves", () => {
    const ctx = new FakeAudioContext();
    const when = 0.5;
    playPatch(
      ctx,
      ctx.destination,
      single(
        voice({
          wave: "pulse",
          pitch: [
            { at: 0, value: 100 },
            { at: 1, value: 200 },
          ],
          width: [{ at: 0, value: 0.5 }],
        }),
      ),
      when,
      "pulse",
    );

    const delays = ctx.ofKind("delay");
    expect(delays).toHaveLength(1);
    const delay = delays[0] as FakeDelay;
    expectEvents(delay.delayTime, [
      ["setValueAtTime", 0.005, when],
      ["setValueAtTime", 0.0025, when + 1],
    ]);
    expect(delay.maxDelayTime).toBeGreaterThanOrEqual(0.0095);
  });

  // Mutation caught: one straight ramp across a pitch sweep (the delay
  // halfway through an exponential sweep from 100 to 400 Hz would read
  // 0.003125, a width of 0.625, instead of 0.0025).
  it("follows a pitch sweep with the delay in short pieces", () => {
    const ctx = new FakeAudioContext();
    playPatch(
      ctx,
      ctx.destination,
      single(
        voice({
          wave: "pulse",
          pitch: [
            { at: 0, value: 100 },
            { at: 1, value: 400, ramp: "exp" },
          ],
          width: [{ at: 0, value: 0.5 }],
        }),
      ),
      0,
      "sweep",
    );
    const delay = ctx.ofKind("delay")[0]!;
    const events = delay.delayTime.events;
    expect(events.length).toBeGreaterThan(10);
    const mid = events.find((e) => Math.abs(e[2] - 0.5) < 1e-9);
    expect(mid?.[0]).toBe("linearRampToValueAtTime");
    expect(mid?.[1]).toBeCloseTo(0.0025, 9);
    const last = events.at(-1)!;
    expect(last[1]).toBeCloseTo(0.00125, 9);
    expect(last[2]).toBeCloseTo(1, 9);
  });

  // Mutation caught: the release starting at 0 (or at the voice start),
  // the stop before the release ends, or the voice offset ignored.
  it("shapes the envelope and stops after the release", () => {
    const ctx = new FakeAudioContext();
    const when = 2;
    const handle = playPatch(
      ctx,
      ctx.destination,
      single(
        voice({
          at: 0.25,
          length: 0.5,
          gain: 0.8,
          env: { a: 0.01, d: 0.1, s: 0.5, r: 0.2 },
        }),
      ),
      when,
      "env",
    );

    const t0 = when + 0.25;
    const envGains = ctx
      .ofKind("gain")
      .filter((g) => g.gain.events[0]?.[0] === "setValueAtTime");
    expect(envGains).toHaveLength(1);
    expectEvents((envGains[0] as FakeNode).params.gain as FakeParam, [
      ["setValueAtTime", 0, t0],
      ["linearRampToValueAtTime", 0.8, t0 + 0.01],
      ["linearRampToValueAtTime", 0.4, t0 + 0.11],
      ["linearRampToValueAtTime", 0.4, t0 + 0.5],
      ["linearRampToValueAtTime", 0, t0 + 0.7],
    ]);
    const oscs = ctx.ofKind("oscillator");
    expect(oscs).toHaveLength(1);
    const osc = oscs[0] as FakeNode;
    expect(osc.started).toEqual([t0]);
    expect(osc.stopped).toHaveLength(1);
    expect(osc.stopped[0]).toBeCloseTo(t0 + 0.7, 9);
    expect(handle.end).toBeCloseTo(when + 0.95, 9);
    // Pitch steps count from the patch start, not the voice start.
    expectEvents(osc.params.frequency as FakeParam, [
      ["setValueAtTime", 220, when],
    ]);
  });

  // Mutation caught: the cutoff steps ignored (the filter left at its
  // default), an exponential step read as linear, or the Q not set.
  it("sweeps the filter", () => {
    const ctx = new FakeAudioContext();
    const when = 0.1;
    playPatch(
      ctx,
      ctx.destination,
      single(
        voice({
          filter: {
            type: "lowpass",
            q: 8,
            cutoff: [
              { at: 0, value: 200 },
              { at: 0.5, value: 2000, ramp: "exp" },
              { at: 1, value: 800, ramp: "linear" },
            ],
          },
        }),
      ),
      when,
      "filter",
    );

    const filters = ctx.ofKind("biquad");
    expect(filters).toHaveLength(1);
    const filter = filters[0]!;
    expect(filter.type).toBe("lowpass");
    expect(filter.Q.value).toBe(8);
    expectEvents(filter.frequency, [
      ["setValueAtTime", 200, when],
      ["exponentialRampToValueAtTime", 2000, when + 0.5],
      ["linearRampToValueAtTime", 800, when + 1],
    ]);
    // The source runs through the filter.
    expect(ctx.ofKind("oscillator")[0]?.connections).toContain(filter);
  });

  // Mutation caught: a built-in wave in place of the organ's partials, or
  // the partials at the wrong harmonics.
  it("voices the organ from its five partials", () => {
    const ctx = new FakeAudioContext();
    playPatch(ctx, ctx.destination, single(voice({ wave: "organ" })), 0, "o");
    const osc = ctx.ofKind("oscillator")[0]!;
    expect(osc.type).toBe("custom");
    const wave = osc.wave as FakeWave;
    expect(wave.real).toEqual([0, 0, 0, 0, 0, 0, 0]);
    expect(wave.imag.map((x) => Math.round(x * 100) / 100)).toEqual([
      0, 1, 0.5, 0.35, 0.25, 0, 0.12,
    ]);
  });

  // Mutation caught: noise from another name than the patch's seed name,
  // or a noise source that plays once instead of looping.
  it("plays noise as a looping seeded buffer", () => {
    const ctx = new FakeAudioContext(8000);
    playPatch(
      ctx,
      ctx.destination,
      single(voice({ wave: "noise", pitch: [] })),
      0,
      "hiss",
    );
    const players = ctx.ofKind("bufferSource");
    expect(players).toHaveLength(1);
    const player = players[0]!;
    expect(player.loop).toBe(true);
    const played = player.buffer!.getChannelData(0);
    expect(played).toHaveLength(16000);
    const expected = noiseBuffer(ctx, 2, "hiss").getChannelData(0);
    expect(Array.from(played)).toEqual(Array.from(expected));
    expect(ctx.ofKind("oscillator")).toHaveLength(0);
  });

  // Mutation caught: a stop with no ramp (a click), a source left
  // running past the fade, or a graph never disconnected.
  it("stop fades out and disconnects", () => {
    const ctx = new FakeAudioContext();
    const dest = ctx.createGain();
    const handle = playPatch(
      ctx,
      dest,
      {
        name: "two",
        voices: [voice({ length: 2 }), voice({ wave: "noise", length: 2 })],
      },
      0,
      "stop",
    );
    ctx.currentTime = 0.3;
    handle.stop(0.04);

    const level = patchGain(ctx, dest);
    const events = (level.params.gain as FakeParam).events;
    const ramp = events.find((e) => e[0] === "linearRampToValueAtTime");
    expect(ramp?.[1]).toBe(0);
    expect(ramp?.[2]).toBeCloseTo(0.34, 9);
    const all = sources(ctx);
    expect(all.length).toBeGreaterThan(0);
    for (const source of all) {
      expect(source.stopped.at(-1)).toBeCloseTo(0.34, 9);
    }
    expect(handle.end).toBeCloseTo(0.34, 9);

    for (const source of all) (source as unknown as { end(): void }).end();
    expect(level.disconnects).toBeGreaterThan(0);
    expect(level.connections).not.toContain(dest);
  });

  // Mutation caught: a stop that lengthens a one-shot already ending
  // sooner than the fade would.
  it("never stops a source later than it was going to end", () => {
    const ctx = new FakeAudioContext();
    const handle = playPatch(
      ctx,
      ctx.destination,
      single(voice({ length: 0.1, env: { a: 0, d: 0, s: 1, r: 0.05 } })),
      0,
      "short",
    );
    ctx.currentTime = 0.14;
    handle.stop(0.5);
    const osc = ctx.ofKind("oscillator")[0]!;
    expect(osc.stopped.at(-1)).toBeCloseTo(0.15, 9);
  });

  // Mutation caught: a drone that ends after its first cycle (a source
  // stop or a release ramp scheduled for a looping patch).
  it("holds a looping patch until stopped", () => {
    const ctx = new FakeAudioContext();
    const handle = playPatch(
      ctx,
      ctx.destination,
      single(voice({ tremolo: { rate: 0.2, depth: 0.5 } }), true),
      0,
      "drone",
    );

    const all = sources(ctx);
    // The voice's oscillator and the tremolo's.
    expect(all).toHaveLength(2);
    for (const source of all) expect(source.stopped).toEqual([]);
    const envGain = ctx
      .ofKind("gain")
      .find((g) => g.gain.events[0]?.[0] === "setValueAtTime");
    expect(envGain).toBeDefined();
    const last = envGain!.gain.events.at(-1)!;
    expect(last[1]).toBeCloseTo(0.5, 9);
    expect(handle.end).toBe(Number.POSITIVE_INFINITY);

    ctx.currentTime = 10;
    handle.stop(0.1);
    for (const source of all) expect(source.stopped).toEqual([10.1]);
  });

  // Mutation caught: the tremolo's LFO not wired into a gain parameter.
  it("wobbles a tremolo voice through a gain parameter", () => {
    const ctx = new FakeAudioContext();
    playPatch(
      ctx,
      ctx.destination,
      single(voice({ tremolo: { rate: 4, depth: 0.5 } })),
      0,
      "trem",
    );
    const lfo = ctx
      .ofKind("oscillator")
      .find((o) => o.frequency.events[0]?.[1] === 4);
    expect(lfo).toBeDefined();
    const depth = lfo!.connections[0] as FakeNode;
    expect(depth.kind).toBe("gain");
    expect(depth.params.gain?.value).toBeCloseTo(0.25, 9);
    const target = depth.connections[0];
    const wobbled = ctx.ofKind("gain").find((g) => g.gain === target);
    expect(wobbled?.gain.value).toBeCloseTo(0.75, 9);
  });
});

describe("noiseBuffer", () => {
  // Mutation caught: `Math.random` (two calls with one name would differ),
  // or a fill that ignores the name.
  it("fills noise from the seed, the same for the same name", () => {
    const ctx = new FakeAudioContext(8000);
    const a = noiseBuffer(ctx, 0.5, "hiss").getChannelData(0);
    const b = noiseBuffer(ctx, 0.5, "hiss").getChannelData(0);
    const c = noiseBuffer(ctx, 0.5, "other").getChannelData(0);
    expect(a).toHaveLength(4000);
    expect(Array.from(a)).toEqual(Array.from(b));
    expect(Array.from(a)).not.toEqual(Array.from(c));
    for (const sample of a) {
      expect(sample).toBeGreaterThanOrEqual(-1);
      expect(sample).toBeLessThanOrEqual(1);
    }
    // Not silence: the samples spread over the range.
    expect(Math.max(...a)).toBeGreaterThan(0.9);
    expect(Math.min(...a)).toBeLessThan(-0.9);
  });
});
