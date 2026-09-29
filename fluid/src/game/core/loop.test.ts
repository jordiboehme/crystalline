import { describe, expect, it } from "vitest";

import { MAX_FRAME_MS, TICK_MS, createLoop, type Clock } from "./loop";

/** A clock the test advances by hand, one animation frame at a time. */
function fakeClock() {
  let time = 0;
  let pending: ((t: number) => void) | null = null;
  const clock: Clock = {
    now: () => time,
    request: (cb) => {
      pending = cb;
      return 1;
    },
    cancel: () => {
      pending = null;
    },
  };
  return {
    clock,
    frame(ms: number) {
      time += ms;
      const cb = pending;
      pending = null;
      cb?.(time);
    },
    /** Delivers the pending frame with timestamp `t`, leaving `now()` alone. */
    frameAt(t: number) {
      const cb = pending;
      pending = null;
      cb?.(t);
    },
    advance(ms: number) {
      time += ms;
    },
    get scheduled() {
      return pending !== null;
    },
  };
}

describe("createLoop", () => {
  it("runs 35 ticks for one second of frames", () => {
    const c = fakeClock();
    let ticks = 0;
    const loop = createLoop({ tick: () => ticks++, render: () => {} }, c.clock);
    loop.start();
    for (let i = 0; i < 60; i++) c.frame(1000 / 60);
    expect(ticks).toBeGreaterThanOrEqual(34);
    expect(ticks).toBeLessThanOrEqual(35);
  });

  it("renders once per frame with an alpha in [0, 1)", () => {
    const c = fakeClock();
    const alphas: number[] = [];
    const loop = createLoop(
      { tick: () => {}, render: (alpha) => alphas.push(alpha) },
      c.clock,
    );
    loop.start();
    for (let i = 0; i < 10; i++) c.frame(7);
    expect(alphas).toHaveLength(10);
    for (const a of alphas) {
      expect(a).toBeGreaterThanOrEqual(0);
      expect(a).toBeLessThan(1);
    }
  });

  it("clamps a long frame so a returning tab does not storm", () => {
    const c = fakeClock();
    let ticks = 0;
    const loop = createLoop({ tick: () => ticks++, render: () => {} }, c.clock);
    loop.start();
    c.frame(16);
    ticks = 0;
    c.frame(10 * 60 * 1000);
    expect(ticks).toBeLessThanOrEqual(Math.ceil(MAX_FRAME_MS / TICK_MS));
  });

  it("stops scheduling after stop()", () => {
    const c = fakeClock();
    const loop = createLoop({ tick: () => {}, render: () => {} }, c.clock);
    loop.start();
    c.frame(16);
    loop.stop();
    expect(c.scheduled).toBe(false);
    expect(loop.running).toBe(false);
  });

  it("ignores a second start()", () => {
    const c = fakeClock();
    let renders = 0;
    const loop = createLoop(
      { tick: () => {}, render: () => renders++ },
      c.clock,
    );
    loop.start();
    loop.start();
    c.frame(16);
    expect(renders).toBe(1);
  });

  it("treats a first frame stamped before start() as no time at all", () => {
    const c = fakeClock();
    let ticks = 0;
    const renders: [number, number][] = [];
    const loop = createLoop(
      {
        tick: () => ticks++,
        render: (alpha, frameMs) => renders.push([alpha, frameMs]),
      },
      c.clock,
    );
    c.advance(1000);
    loop.start();
    c.frameAt(999.5);
    expect(ticks).toBe(0);
    expect(renders).toHaveLength(1);
    const [alpha, frameMs] = renders[0] ?? [NaN, NaN];
    expect(alpha).toBeGreaterThanOrEqual(0);
    expect(frameMs).toBe(0);
  });
});
