/**
 * The station's heartbeat: logic at DOOM's 35 Hz, pictures as fast as the
 * display asks for them.
 *
 * Movement, light specials and every other rule advance in fixed 1/35 s
 * ticks, so they behave the same on a 60 Hz laptop and a 144 Hz monitor, and
 * DOOM's light constants (counted in tics) can be used as written. Rendering
 * runs on `requestAnimationFrame` and receives how far the clock is between
 * the last tick and the next one, so the camera can be interpolated and does
 * not stutter at 35 steps a second.
 *
 * A frame longer than `MAX_FRAME_MS` is cut short. A tab that was hidden for
 * ten minutes would otherwise come back owing twenty thousand ticks and run
 * them all in one frame; cutting the debt simply pauses the world while
 * nobody was looking.
 */

/** Logic ticks per second, DOOM's `TICRATE`. */
export const TICK_HZ = 35;

/** The length of one tick in milliseconds. */
export const TICK_MS = 1000 / TICK_HZ;

/** The longest stretch of time one frame may account for. */
export const MAX_FRAME_MS = 250;

/** Where time and frames come from; the browser's in the app, a fake in tests. */
export interface Clock {
  now(): number;
  request(cb: (t: number) => void): number;
  cancel(id: number): void;
}

/** What the loop drives. */
export interface LoopHooks {
  /** One fixed logic step. */
  tick(): void;
  /** One picture. `alpha` is the fraction of a tick since the last one, `frameMs` the real frame time. */
  render(alpha: number, frameMs: number): void;
}

/** A running or stopped loop. */
export interface Loop {
  start(): void;
  stop(): void;
  readonly running: boolean;
}

const browserClock: Clock = {
  now: () => performance.now(),
  request: (cb) => requestAnimationFrame(cb),
  cancel: (id) => cancelAnimationFrame(id),
};

/** A fixed-step loop over `hooks`. Nothing runs until `start()`. */
export function createLoop(
  hooks: LoopHooks,
  clock: Clock = browserClock,
): Loop {
  let handle: number | null = null;
  let last = 0;
  let debt = 0;

  const frame = (now: number) => {
    const frameMs = Math.min(now - last, MAX_FRAME_MS);
    last = now;
    debt += frameMs;
    while (debt >= TICK_MS) {
      hooks.tick();
      debt -= TICK_MS;
    }
    hooks.render(debt / TICK_MS, frameMs);
    if (handle !== null) handle = clock.request(frame);
  };

  return {
    start() {
      if (handle !== null) return;
      last = clock.now();
      debt = 0;
      handle = clock.request(frame);
    },
    stop() {
      if (handle === null) return;
      clock.cancel(handle);
      handle = null;
    },
    get running() {
      return handle !== null;
    },
  };
}
