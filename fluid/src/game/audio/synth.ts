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
 * One voice is: its source(s), then the filter if it has one, then its
 * envelope gain, then a tremolo gain if it has one (a sine LFO feeding
 * that gain's parameter). Every voice meets in the patch's own level gain,
 * which feeds `dest`. Stopping a patch ramps that level to zero (no click)
 * and stops every source when the ramp ends; once the last source has
 * ended, the whole graph is disconnected.
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
  ScheduledSourceLike,
} from "./context";
import {
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

/** The piece length of the pulse's delay where the pitch ramps. */
const DELAY_PIECE_S = 0.02;

/** The smallest target an exponential ramp is given (WebAudio throws on 0). */
const MIN_EXP = 1e-4;

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
 * voices play `noiseBuffer(ctx, NOISE_BUFFER_S, seedName)`. The returned
 * stop fades out in `fadeS`.
 */
export function playPatch(
  ctx: BaseAudioContextLike,
  dest: AudioNodeLike,
  patch: Patch,
  when: number,
  seedName: string,
): PlayingPatch {
  const loop = patch.loop === true;
  const made: AudioNodeLike[] = [];
  const sources: Source[] = [];
  const level = ctx.createGain();
  level.gain.value = 1;
  level.connect(dest);
  made.push(level);

  let noise: AudioBufferLike | null = null;
  const noiseOf = (): AudioBufferLike =>
    (noise ??= noiseBuffer(ctx, NOISE_BUFFER_S, seedName));

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

    const { play: players, outs } = sourcesOf(ctx, voice, when, noiseOf, track);
    for (const node of outs) node.connect(input);
    for (const node of players) play(node);
  }

  let stopped = false;
  let end = loop ? Number.POSITIVE_INFINITY : when + patchLength(patch);
  let ended = 0;
  const release = (): void => {
    ended += 1;
    if (ended < sources.length) return;
    for (const node of made) node.disconnect();
  };
  for (const source of sources) source.node.onended = release;

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

/** A voice's sources (to start and stop) and its outputs (to connect). */
interface VoiceSources {
  play: ScheduledSourceLike[];
  outs: AudioNodeLike[];
}

/**
 * The sources of one voice: one oscillator, the pulse's two saws, or a
 * noise player. For the pulse the outputs are the first saw and the delay
 * line the second saw already feeds through its inverter.
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
    return { play: [player], outs: [player] };
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
      return { play: [node], outs: [node] };
    }
    case "organ": {
      const node = osc();
      const real = new Float32Array(ORGAN_PARTIALS.length);
      const imag = Float32Array.from(ORGAN_PARTIALS);
      node.setPeriodicWave(ctx.createPeriodicWave(real, imag));
      return { play: [node], outs: [node] };
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
      scheduleDelay(delay.delayTime, width, pitch, when);
      inverted.connect(flip);
      flip.connect(delay);
      return { play: [direct, inverted], outs: [direct, delay] };
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

/** Schedules a step list on `param`, its times counted from `when`. */
function applySteps(
  param: AudioParamLike,
  steps: readonly Step[],
  when: number,
): void {
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

/**
 * Schedules the pulse's delay, `width / pitch`, from `when`. At each step
 * time of either list: a ramp to the value the segment before it arrives
 * at (when either list moves in that segment), then a jump to the new
 * value when a step jumps. A segment in which the pitch moves is cut into
 * `DELAY_PIECE_S` pieces, since 1 / pitch is no straight line.
 */
function scheduleDelay(
  param: AudioParamLike,
  width: readonly Step[],
  pitch: readonly Step[],
  when: number,
): void {
  const delayAt = (w: number, p: number): number =>
    Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, w)) / Math.max(MIN_HZ, p);
  const times = [...new Set([...width, ...pitch].map((s) => s.at))].sort(
    (a, b) => a - b,
  );
  let prev: number | null = null;
  for (const t of times) {
    const right = delayAt(valueAt(width, t), valueAt(pitch, t));
    if (prev === null) {
      param.setValueAtTime(right, when + t);
      prev = t;
      continue;
    }
    const pitchMoves = movesBefore(pitch, t);
    if (pitchMoves || movesBefore(width, t)) {
      if (pitchMoves) {
        const pieces = Math.max(1, Math.ceil((t - prev) / DELAY_PIECE_S));
        for (let j = 1; j < pieces; j++) {
          const tj = prev + ((t - prev) * j) / pieces;
          param.linearRampToValueAtTime(
            delayAt(valueAt(width, tj), valueAt(pitch, tj)),
            when + tj,
          );
        }
      }
      const left = delayAt(valueBefore(width, t), valueBefore(pitch, t));
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

/** A step list's value just before `t`: what a ramp arriving at `t` reaches. */
function valueBefore(steps: readonly Step[], t: number): number {
  const i = steps.findIndex((s) => s.at >= t);
  if (i < 0) return valueAt(steps, t);
  const next = steps[i] as Step;
  if (i === 0) return next.value;
  const prev = steps[i - 1] as Step;
  if (next.ramp === undefined) return prev.value;
  return valueAt([prev, next], t);
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
