/**
 * The mixer: one audio context, four buses, a master gain and the mute.
 *
 * Every sound goes into one of four buses (`effects`, `signature`,
 * `ambience`, `modem`), each a gain at its ruled level, and every bus into
 * one master gain into the context's destination (M4 C21). Muting ramps
 * the master to zero in 30 ms (no click) and remembers the choice in
 * `localStorage` under `MUTE_KEY`, as the inverted look remembers its own;
 * a storage that refuses still toggles for the session.
 *
 * **Where the context comes from (`ContextSource`).** Browsers start a
 * context only inside a user gesture, and the game's code runs after its
 * lazy chunk has arrived, outside the gesture that launched it. So a
 * context is either:
 * - **borrowed**: one someone else made in the gesture and owns (the
 *   launch's primed context). The mixer takes it at creation, uses it and
 *   never closes it; on `close` it only suspends it and hands it back, so
 *   a second mount (React's development remount) still finds it alive.
 * - **made**: when nothing was borrowed, the mixer makes its own on the
 *   first `unlock` (called from a click or a key, inside that gesture),
 *   never at creation. It owns that one and closes it on `close`.
 * With neither (no WebAudio at all, a factory that throws) the mixer is
 * silent: `ctx` and every `bus` are null and every method does nothing,
 * so the game runs the same without sound. No method ever throws, and a
 * refused `resume` or `suspend` is swallowed (Review Focus 5).
 *
 * `unlock`, `resume` and `suspend` call through to the context whatever
 * its `state` reads. Both calls are idempotent, and `state` changes only
 * when the promise settles, so a gate on it would skip a pause that comes
 * right after an unlock still settling (or a resume after a suspend still
 * settling). It would also never wake a context Safari marked
 * `"interrupted"`, which is neither running nor suspended. Only a closed
 * context is left alone: it cannot come back.
 */

import type {
  AudioContextLike,
  AudioNodeLike,
  AudioParamLike,
  GainLike,
} from "./context";

/** The mixer's buses, one per kind of sound. */
export type Bus = "effects" | "signature" | "ambience" | "modem";

/**
 * Each bus's level (M4 C21): the modem sits below the effects and the
 * ambience well below everything, so a drone never masks a door.
 */
export const BUS_LEVELS = {
  effects: 1.0,
  signature: 0.9,
  ambience: 0.22,
  modem: 0.45,
} as const satisfies Record<Bus, number>;

/** The master level every bus runs through (M4 C21). */
export const MASTER_LEVEL = 0.7;

/** Where the mute choice is remembered: `"1"` muted, anything else not. */
export const MUTE_KEY = "station.muted";

/** How long the master takes to fall silent or come back on a mute. */
export const MUTE_RAMP_S = 0.03;

/** Where a mixer's context comes from; both default to none. */
export interface ContextSource {
  /** A context someone else owns (the launch's primed one): used, never closed. */
  borrow?: () => AudioContextLike | null;
  /** A context the mixer makes on `unlock` and owns: closed on `close`. */
  make?: () => AudioContextLike | null;
}

/** The game's sound output. */
export interface Mixer {
  /** The context, or null when the browser has none (or the mixer closed). */
  readonly ctx: AudioContextLike | null;
  /** The gain to play into for `bus`, or null while there is no context. */
  bus(bus: Bus): AudioNodeLike | null;
  /** Whether the context is running (sound can be heard). */
  readonly running: boolean;
  /** From a user gesture: resumes the context, making one when there is none yet. */
  unlock(): void;
  /** Suspends the context (the pause, a hidden tab), whatever its state reads. */
  suspend(): void;
  /**
   * Resumes the context, also an interrupted one, whatever its state reads;
   * never makes one (it may run outside a gesture).
   */
  resume(): void;
  /** Whether sound is muted. */
  readonly muted: boolean;
  /** Toggles and remembers; returns the new state. */
  toggleMute(): boolean;
  /** Disconnects the graph; closes an owned context, suspends a borrowed one. */
  close(): void;
}

/** The nodes the mixer built on its context. */
interface Graph {
  master: GainLike;
  buses: Record<Bus, GainLike>;
}

/**
 * A mixer over `source`. It takes `source.borrow?.()` at creation;
 * `unlock()` calls `source.make?.()` when it still has none.
 */
export function createMixer(source: ContextSource = {}): Mixer {
  let muted = readMuted();
  let closed = false;
  let owned = false;
  let ctx: AudioContextLike | null = attempt(source.borrow);
  let graph: Graph | null = ctx === null ? null : build(ctx, muted);

  const quietly = (run: (c: AudioContextLike) => Promise<void>): void => {
    if (ctx === null || ctx.state === "closed") return;
    try {
      void run(ctx).catch(() => undefined);
    } catch {
      // A context that throws instead of rejecting: stay silent.
    }
  };

  return {
    get ctx() {
      return ctx;
    },
    bus(bus) {
      return graph?.buses[bus] ?? null;
    },
    get running() {
      return ctx !== null && ctx.state === "running";
    },
    unlock() {
      if (closed) return;
      if (ctx === null) {
        ctx = attempt(source.make);
        if (ctx === null) return;
        owned = true;
        graph = build(ctx, muted);
      }
      quietly((c) => c.resume());
    },
    suspend() {
      quietly((c) => c.suspend());
    },
    resume() {
      quietly((c) => c.resume());
    },
    get muted() {
      return muted;
    },
    toggleMute() {
      muted = !muted;
      writeMuted(muted);
      if (ctx !== null && graph !== null) {
        rampTo(graph.master.gain, muted ? 0 : MASTER_LEVEL, ctx.currentTime);
      }
      return muted;
    },
    close() {
      if (closed) return;
      closed = true;
      if (graph !== null) {
        for (const bus of Object.values(graph.buses)) bus.disconnect();
        graph.master.disconnect();
      }
      if (owned) quietly((c) => c.close());
      else quietly((c) => c.suspend());
      graph = null;
      ctx = null;
    },
  };
}

/**
 * A live context of the browser's own, for `ContextSource.make`: called
 * inside the gesture that unlocks. Null without WebAudio (jsdom, an old
 * browser); a constructor that throws is the mixer's to catch.
 */
export function makeAudioContext(): AudioContext | null {
  const Ctor = (globalThis as { AudioContext?: new () => AudioContext })
    .AudioContext;
  return Ctor === undefined ? null : new Ctor();
}

/** The master and the four buses on `ctx`, the master at the mute state. */
function build(ctx: AudioContextLike, muted: boolean): Graph {
  const master = ctx.createGain();
  master.gain.value = muted ? 0 : MASTER_LEVEL;
  master.connect(ctx.destination);
  const bus = (name: Bus): GainLike => {
    const gain = ctx.createGain();
    gain.gain.value = BUS_LEVELS[name];
    gain.connect(master);
    return gain;
  };
  return {
    master,
    buses: {
      effects: bus("effects"),
      signature: bus("signature"),
      ambience: bus("ambience"),
      modem: bus("modem"),
    },
  };
}

/** Ramps `param` from where it is now to `target` over `MUTE_RAMP_S`. */
function rampTo(param: AudioParamLike, target: number, now: number): void {
  param.cancelScheduledValues(now);
  param.setValueAtTime(param.value, now);
  param.linearRampToValueAtTime(target, now + MUTE_RAMP_S);
}

/** Calls a context factory, turning a throw into none. */
function attempt(
  factory: (() => AudioContextLike | null) | undefined,
): AudioContextLike | null {
  try {
    return factory?.() ?? null;
  } catch {
    return null;
  }
}

/** Reads the mute choice; anything but a readable "1" is sound on. */
function readMuted(): boolean {
  try {
    return window.localStorage.getItem(MUTE_KEY) === "1";
  } catch {
    return false;
  }
}

/** Remembers the mute choice, when the storage lets it. */
function writeMuted(muted: boolean): void {
  try {
    window.localStorage.setItem(MUTE_KEY, muted ? "1" : "0");
  } catch {
    // A private window or blocked storage: the choice lasts this session.
  }
}
