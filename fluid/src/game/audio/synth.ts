/**
 * The synth: turns a patch (`audio/patch.ts`) into WebAudio nodes.
 *
 * SID-style here means three things, each built from the plain nodes every
 * browser has:
 *
 * - **The width-modulated pulse.** WebAudio's square wave has a fixed 50 %
 *   duty cycle, so the pulse is made the classic way: two sawtooth
 *   oscillators at the same frequency, the second inverted (a gain of -1)
 *   and delayed through a delay line. The sum is a pulse whose duty cycle
 *   is the delay divided by the period, so the delay is `width(t) /
 *   pitch(t)`. It is scheduled at the union of the width's and the pitch's
 *   step times (so a pitch step keeps the width, rather than changing it
 *   with the period), ramped linearly where the width alone moves (the
 *   delay is then linear too) and in 20 ms pieces where the pitch moves
 *   (the delay then follows 1 / pitch, which a single line would bend).
 *   The line is made long enough for the lowest pitch at the widest pulse.
 *   The two halves meet in one gain that levels the pulse with the other
 *   waves: a pulse of width w swings between 2w and 2w - 2, so its peak is
 *   `2 * max(w, 1 - w)` (1 for a square, 1.8 at width 0.1), and the gain is
 *   the inverse of that, scheduled from the width the same way.
 * - **The resonant filter swept over time.** A biquad whose cutoff follows
 *   the voice's step list, its resonance the voice's `q`.
 * - **50 Hz arpeggios.** Nothing to build: an arpeggio is a pitch step list
 *   (`arpeggio`), scheduled like any other.
 *
 * The other waves: `saw`, `triangle` and `sine` are the built-in
 * oscillator types; `organ` a periodic wave of partials 1, 2, 3, 4 and 6 at
 * 1, 0.5, 0.35, 0.25 and 0.12, a drawbar organ; `noise` a looping buffer of
 * seeded noise (`noiseBuffer`), never an unseeded random, so a render is the
 * same every time (M4 C29).
 *
 * Two slow modulations keep a looping drone alive, since a loop holds its
 * steps' last values: `drift`, a sine LFO into the `detune` of every
 * oscillator of the voice (both saws of a pulse from one LFO gain, or the
 * two halves would beat against each other), and `pwm`, a sine LFO added
 * to the pulse's delay line, whose depth in seconds is the width depth over
 * the voice's first pitch (a drone's pitch holds, so that is its period).
 *
 * The noise buffer is filled once per context and name and reused (up to
 * `NOISE_CACHE_SIZE` names per context), so a footstep does not fill two
 * seconds of samples on the main thread; several sources may play one
 * buffer at once.
 *
 * Every parameter a step list drives holds its first step's value from the
 * patch start, so nothing sounds at a node's default (440 Hz, a delay of
 * 0) before the first step, and `valueAt` reads the schedule as it plays.
 * An exponential step is floored at `MIN_EXP`, since WebAudio throws on a
 * ramp to 0.
 *
 * One voice is: its source(s), then the filter if it has one, then its
 * envelope gain, then a tremolo gain if it has one (a sine LFO feeding
 * that gain's parameter). Every voice meets in the patch's own level gain,
 * which feeds `dest`. Stopping a patch ramps that level to zero (no click)
 * and stops every source when the ramp ends; once the last source has
 * ended, the whole graph is disconnected (a patch with no voices at once).
 *
 * Only the base context interface is used, so a patch renders the same on
 * a live context and an offline one.
 */

import { createRng, seedFor } from "../core/seed";
import type {
  AudioBufferLike,
  AudioNodeLike,
  AudioParamLike,
  BaseAudioContextLike,
  DelayLike,
  OscillatorLike,
  ScheduledSourceLike,
} from "./context";
import {
  MIN_EXP,
  patchLength,
  valueAt,
  type Env,
  type Patch,
  type Step,
  type Voice,
} from "./patch";

/** The fade a `stop()` with no argument uses: short, but no click. */
export const STOP_FADE_S = 0.03;

/** How long a noise voice's looping buffer is. */
export const NOISE_BUFFER_S = 2;

/** The pitch of a voice that names none (and is not noise). */
const DEFAULT_PITCH: readonly Step[] = [{ at: 0, value: 440 }];

/** The width of a pulse that names none: a square. */
const DEFAULT_WIDTH: readonly Step[] = [{ at: 0, value: 0.5 }];

/** The narrowest and widest pulse. */
const MIN_WIDTH = 0.05;
const MAX_WIDTH = 0.95;

/** No pitch below this reaches the delay's arithmetic (1 Hz). */
const MIN_HZ = 1;

/** Headroom on the pulse's delay line beyond the longest delay it needs. */
const DELAY_MARGIN_S = 0.01;

/**
 * The piece length of a schedule derived from step lists (the pulse's
 * delay and level) where an input moves along a curve.
 */
const PIECE_S = 0.02;

/** How many noise buffers a context keeps, by name, before the oldest goes. */
export const NOISE_CACHE_SIZE = 16;

/** The noise buffers filled so far, per context and name. */
const noiseCache = new WeakMap<
  BaseAudioContextLike,
  Map<string, AudioBufferLike>
>();

/**
 * The noise buffer for `name` on `ctx`: the one filled before, or a new
 * one, the oldest name dropped once the context holds `NOISE_CACHE_SIZE`.
 */
function cachedNoise(ctx: BaseAudioContextLike, name: string): AudioBufferLike {
  let byName = noiseCache.get(ctx);
  if (byName === undefined) {
    byName = new Map();
    noiseCache.set(ctx, byName);
  }
  const held = byName.get(name);
  if (held !== undefined) return held;
  const made = noiseBuffer(ctx, NOISE_BUFFER_S, name);
  if (byName.size >= NOISE_CACHE_SIZE) {
    const oldest = byName.keys().next().value;
    if (oldest !== undefined) byName.delete(oldest);
  }
  byName.set(name, made);
  return made;
}

/** The organ's partials: index is the harmonic, value its sine amplitude. */
const ORGAN_PARTIALS = [0, 1, 0.5, 0.35, 0.25, 0, 0.12] as const;

/** A playing patch. */
export interface PlayingPatch {
  /**
   * Fades the patch out over `fadeS` (default `STOP_FADE_S`) from the
   * context's current time and stops every source when the fade ends
   * (never later than the source would have ended). A second call does
   * nothing.
   */
  stop(fadeS?: number): void;
  /**
   * When the patch falls silent, in context seconds: `when` plus its
   * length, `Infinity` for a looping patch not yet stopped, and the end of
   * the fade once stopped.
   */
  readonly end: number;
}

/** One started source and when it is due to stop on its own. */
interface Source {
  node: ScheduledSourceLike;
  end: number;
}

/**
 * Schedules `patch` on `ctx` at `when` (context seconds) into `dest`. Noise
 * voices play `noiseBuffer(ctx, NOISE_BUFFER_S, seedName)` (kept per
 * context and name). The returned stop fades out in `fadeS`. `onEnd` is
 * called once, when the last source has ended and the graph is
 * disconnected (at once for a patch with no voices), so a caller can take
 * down what it put between the patch and its bus.
 */
export function playPatch(
  ctx: BaseAudioContextLike,
  dest: AudioNodeLike,
  patch: Patch,
  when: number,
  seedName: string,
  onEnd?: () => void,
): PlayingPatch {
  const loop = patch.loop === true;
  const made: AudioNodeLike[] = [];
  const sources: Source[] = [];
  const level = ctx.createGain();
  level.connect(dest);
  made.push(level);

  const noiseOf = (): AudioBufferLike => cachedNoise(ctx, seedName);

  for (const voice of patch.voices) {
    const start = when + voice.at;
    const end = loop
      ? Number.POSITIVE_INFINITY
      : start + voice.length + voice.env.r;
    const track = <T extends AudioNodeLike>(node: T): T => {
      made.push(node);
      return node;
    };
    const play = (node: ScheduledSourceLike): void => {
      track(node);
      node.start(start);
      if (Number.isFinite(end)) node.stop(end);
      sources.push({ node, end });
    };

    // The chain from the voice's input to the patch level.
    const env = track(ctx.createGain());
    envelope(env.gain, voice.env, voice.gain, start, voice.length, loop);
    let out: AudioNodeLike = env;
    if (voice.tremolo !== undefined) {
      const { rate, depth } = voice.tremolo;
      const wobble = track(ctx.createGain());
      wobble.gain.value = 1 - depth / 2;
      const lfo = ctx.createOscillator();
      lfo.type = "sine";
      lfo.frequency.setValueAtTime(rate, start);
      const amount = track(ctx.createGain());
      amount.gain.value = depth / 2;
      lfo.connect(amount);
      amount.connect(wobble.gain);
      env.connect(wobble);
      play(lfo);
      out = wobble;
    }
    out.connect(level);
    let input: AudioNodeLike = env;
    if (voice.filter !== undefined) {
      const filter = track(ctx.createBiquadFilter());
      filter.type = voice.filter.type;
      filter.Q.value = voice.filter.q;
      applySteps(filter.frequency, voice.filter.cutoff, when);
      filter.connect(env);
      input = filter;
    }

    const {
      play: players,
      outs,
      oscillators,
      delay,
    } = sourcesOf(ctx, voice, when, noiseOf, track);
    for (const node of outs) node.connect(input);
    for (const node of players) play(node);
    // A slow sine LFO into `targets` through one gain of `amount`.
    const lfoInto = (
      rate: number,
      amount: number,
      targets: readonly AudioParamLike[],
    ): void => {
      if (targets.length === 0) return;
      const lfo = ctx.createOscillator();
      lfo.type = "sine";
      lfo.frequency.setValueAtTime(rate, start);
      const depth = track(ctx.createGain());
      depth.gain.value = amount;
      lfo.connect(depth);
      for (const target of targets) depth.connect(target);
      play(lfo);
    };
    if (voice.drift !== undefined) {
      lfoInto(
        voice.drift.rate,
        voice.drift.cents,
        oscillators.map((o) => o.detune),
      );
    }
    if (voice.pwm !== undefined && delay !== null) {
      const hz = Math.max(MIN_HZ, voice.pitch[0]?.value ?? MIN_HZ);
      lfoInto(voice.pwm.rate, voice.pwm.depth / hz, [delay.delayTime]);
    }
  }

  let stopped = false;
  let end = loop ? Number.POSITIVE_INFINITY : when + patchLength(patch);
  let ended = 0;
  let released = false;
  const release = (): void => {
    ended += 1;
    if (released || ended < sources.length) return;
    released = true;
    for (const node of made) node.disconnect();
    onEnd?.();
  };
  for (const source of sources) source.node.onended = release;
  if (sources.length === 0) {
    released = true;
    level.disconnect();
    onEnd?.();
  }

  return {
    get end() {
      return end;
    },
    stop(fadeS = STOP_FADE_S) {
      if (stopped) return;
      stopped = true;
      const now = ctx.currentTime;
      const quiet = now + Math.max(0, fadeS);
      level.gain.cancelScheduledValues(now);
      level.gain.setValueAtTime(level.gain.value, now);
      level.gain.linearRampToValueAtTime(0, quiet);
      let last = now;
      for (const source of sources) {
        const at = Math.min(source.end, quiet);
        source.node.stop(at);
        last = Math.max(last, at);
      }
      end = Math.min(end, last);
    },
  };
}

/**
 * A voice's sources (to start and stop), its outputs (to connect), its
 * oscillators (the drift's targets) and a pulse's delay line (the width
 * modulation's target, null for any other wave).
 */
interface VoiceSources {
  play: ScheduledSourceLike[];
  outs: AudioNodeLike[];
  oscillators: OscillatorLike[];
  delay: DelayLike | null;
}

/**
 * The sources of one voice: one oscillator, the pulse's two saws, or a
 * noise player. For the pulse the output is the levelling gain in which
 * the first saw and the delay line (fed by the second saw through its
 * inverter) meet.
 */
function sourcesOf(
  ctx: BaseAudioContextLike,
  voice: Voice,
  when: number,
  noiseOf: () => AudioBufferLike,
  track: <T extends AudioNodeLike>(node: T) => T,
): VoiceSources {
  if (voice.wave === "noise") {
    const player = ctx.createBufferSource();
    player.buffer = noiseOf();
    player.loop = true;
    return { play: [player], outs: [player], oscillators: [], delay: null };
  }
  const pitch = voice.pitch.length > 0 ? voice.pitch : DEFAULT_PITCH;
  const osc = (): ReturnType<BaseAudioContextLike["createOscillator"]> => {
    const node = ctx.createOscillator();
    applySteps(node.frequency, pitch, when);
    return node;
  };
  switch (voice.wave) {
    case "saw":
    case "triangle":
    case "sine": {
      const node = osc();
      node.type = voice.wave === "saw" ? "sawtooth" : voice.wave;
      return { play: [node], outs: [node], oscillators: [node], delay: null };
    }
    case "organ": {
      const node = osc();
      const real = new Float32Array(ORGAN_PARTIALS.length);
      const imag = Float32Array.from(ORGAN_PARTIALS);
      node.setPeriodicWave(ctx.createPeriodicWave(real, imag));
      return { play: [node], outs: [node], oscillators: [node], delay: null };
    }
    case "pulse": {
      const width =
        voice.width !== undefined && voice.width.length > 0
          ? voice.width
          : DEFAULT_WIDTH;
      const direct = osc();
      direct.type = "sawtooth";
      const inverted = osc();
      inverted.type = "sawtooth";
      const lowest = Math.max(
        MIN_HZ,
        Math.min(...pitch.map((step) => step.value)),
      );
      const flip = track(ctx.createGain());
      flip.gain.value = -1;
      const delay = track(ctx.createDelay(MAX_WIDTH / lowest + DELAY_MARGIN_S));
      scheduleDerived(
        delay.delayTime,
        [width, pitch],
        [pitch],
        (read) => clampWidth(read(width)) / Math.max(MIN_HZ, read(pitch)),
        when,
      );
      const sum = track(ctx.createGain());
      scheduleDerived(
        sum.gain,
        [width],
        [width],
        (read) => {
          const w = clampWidth(read(width));
          return 1 / (2 * Math.max(w, 1 - w));
        },
        when,
      );
      inverted.connect(flip);
      flip.connect(delay);
      direct.connect(sum);
      delay.connect(sum);
      return {
        play: [direct, inverted],
        outs: [sum],
        oscillators: [direct, inverted],
        delay,
      };
    }
  }
}

/**
 * The envelope on a voice's gain from `start`: up to `gain` over `a`, down
 * to `gain * s` over `d`, held to the end of the gate, down to 0 over `r`.
 * A gate shorter than the attack and decay releases from wherever the
 * level got to. A looping voice stays at its sustain level.
 */
function envelope(
  param: AudioParamLike,
  env: Env,
  gain: number,
  start: number,
  length: number,
  loop: boolean,
): void {
  const sustain = gain * env.s;
  param.setValueAtTime(0, start);
  if (loop) {
    param.linearRampToValueAtTime(gain, start + env.a);
    param.linearRampToValueAtTime(sustain, start + env.a + env.d);
    return;
  }
  if (length <= env.a) {
    const reached = env.a > 0 ? (gain * length) / env.a : gain;
    param.linearRampToValueAtTime(reached, start + length);
  } else if (length <= env.a + env.d) {
    param.linearRampToValueAtTime(gain, start + env.a);
    const f = env.d > 0 ? (length - env.a) / env.d : 1;
    param.linearRampToValueAtTime(gain + (sustain - gain) * f, start + length);
  } else {
    param.linearRampToValueAtTime(gain, start + env.a);
    param.linearRampToValueAtTime(sustain, start + env.a + env.d);
    param.linearRampToValueAtTime(sustain, start + length);
  }
  param.linearRampToValueAtTime(0, start + length + env.r);
}

/**
 * Schedules a step list on `param`, its times counted from `when`, holding
 * the first value from `when` on.
 */
function applySteps(
  param: AudioParamLike,
  steps: readonly Step[],
  when: number,
): void {
  const first = steps[0];
  if (first !== undefined && first.at > 0) {
    param.setValueAtTime(first.value, when);
  }
  steps.forEach((step, i) => {
    const time = when + step.at;
    if (i === 0 || step.ramp === undefined) {
      param.setValueAtTime(step.value, time);
    } else if (step.ramp === "linear") {
      param.linearRampToValueAtTime(step.value, time);
    } else {
      param.exponentialRampToValueAtTime(Math.max(step.value, MIN_EXP), time);
    }
  });
}

/** A pulse width inside the narrowest and widest pulse. */
function clampWidth(w: number): number {
  return Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, w));
}

/** Reads one step list at the moment a derived value is computed for. */
type Read = (steps: readonly Step[]) => number;

/**
 * Schedules a value derived from step lists (`compute`) on `param`, from
 * `when`: held at its start value from `when`, then at each step time of
 * any list a ramp to the value the segment before it arrives at (when a
 * list moves in that segment) and a jump to the new value when a step
 * jumps. A segment in which one of the `curved` lists moves is cut into
 * `PIECE_S` pieces, since `compute` bends there (the delay's 1 / pitch,
 * the level's 1 / width); elsewhere it is linear in what moves.
 */
function scheduleDerived(
  param: AudioParamLike,
  lists: readonly (readonly Step[])[],
  curved: readonly (readonly Step[])[],
  compute: (read: Read) => number,
  when: number,
): void {
  const at = (t: number): number => compute((steps) => valueAt(steps, t));
  const times = [
    ...new Set(lists.flatMap((steps) => steps.map((s) => s.at))),
  ].sort((a, b) => a - b);
  let prev: number | null = null;
  for (const t of times) {
    const right = at(t);
    if (prev === null) {
      if (t > 0) param.setValueAtTime(right, when);
      param.setValueAtTime(right, when + t);
      prev = t;
      continue;
    }
    const bends = curved.some((steps) => movesBefore(steps, t));
    if (bends || lists.some((steps) => movesBefore(steps, t))) {
      if (bends) {
        const pieces = Math.max(1, Math.ceil((t - prev) / PIECE_S));
        for (let j = 1; j < pieces; j++) {
          const tj = prev + ((t - prev) * j) / pieces;
          param.linearRampToValueAtTime(at(tj), when + tj);
        }
      }
      const left = compute((steps) => valueBefore(steps, t));
      param.linearRampToValueAtTime(left, when + t);
      if (Math.abs(right - left) > 1e-12) param.setValueAtTime(right, when + t);
    } else {
      param.setValueAtTime(right, when + t);
    }
    prev = t;
  }
}

/** Whether a step list ramps in the segment that ends at `t`. */
function movesBefore(steps: readonly Step[], t: number): boolean {
  const i = steps.findIndex((s) => s.at >= t);
  return i > 0 && steps[i]?.ramp !== undefined;
}

/**
 * A step list's value just before `t`: what a ramp arriving at `t`
 * reaches, or what was held until a step that jumps at `t`.
 */
function valueBefore(steps: readonly Step[], t: number): number {
  const i = steps.findIndex((s) => s.at >= t);
  if (i < 0) return valueAt(steps, t);
  const next = steps[i] as Step;
  if (i === 0) return next.value;
  if (next.at > t || next.ramp !== undefined) {
    return valueAt(steps.slice(0, i + 1), t);
  }
  return valueAt(steps.slice(0, i), t);
}

/**
 * A mono noise buffer of `seconds` at the context's rate, every sample in
 * [-1, 1), filled from `createRng(seedFor("noise", name))`: the same name
 * always gives the same noise (M4 C29).
 */
export function noiseBuffer(
  ctx: BaseAudioContextLike,
  seconds: number,
  name: string,
): AudioBufferLike {
  const length = Math.max(1, Math.ceil(seconds * ctx.sampleRate));
  const buffer = ctx.createBuffer(1, length, ctx.sampleRate);
  const data = buffer.getChannelData(0);
  const rng = createRng(seedFor("noise", name));
  for (let i = 0; i < length; i++) data[i] = rng.range(-1, 1);
  return buffer;
}
