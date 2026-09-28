/**
 * The small slice of WebAudio the station's sound uses, as interfaces.
 *
 * The synth and the mixer never name `AudioContext` or `AudioNode`. They
 * speak to these shapes instead, for three reasons:
 *
 * - Tests run under jsdom, which has no WebAudio at all. A recording fake
 *   (`audio/testContext.ts`) implements the same shapes, so every schedule
 *   the synth writes can be read back and asserted without a sound card.
 * - A patch has to render into an `OfflineAudioContext` as well as a live
 *   one: the offline render is how a patch is listened to and measured
 *   outside the game. The two contexts share a base but differ in their
 *   lifecycle, so there are two interfaces. `BaseAudioContextLike` is what
 *   both have (the node factories, the clock, the destination) and is all
 *   the synth needs. `AudioContextLike` adds the live context's lifecycle
 *   (`state`, `resume`, `suspend`, `close`), which only the mixer needs. An
 *   offline context has no `close()` and its `suspend(time)` needs an
 *   argument, so it fits the base and never the live interface.
 * - Only what is used is listed, which keeps the fake small and makes the
 *   synth's use of the API visible in one place.
 *
 * Methods are declared as methods (not as function-typed properties) so the
 * DOM's own classes fit them: TypeScript compares method parameters
 * bivariantly, which is what lets `AudioNode.connect(AudioNode)` fit
 * `connect(AudioNodeLike)`. `context.test.ts` pins that the real classes
 * satisfy these shapes, at the type level only.
 */

/** A schedulable parameter: a gain, a frequency, a delay time. */
export interface AudioParamLike {
  /** The current value; setting it is an immediate change. */
  value: number;
  /** Jumps to `value` at `time`. */
  setValueAtTime(value: number, time: number): AudioParamLike;
  /** Ramps linearly from the previous event to `value`, arriving at `time`. */
  linearRampToValueAtTime(value: number, time: number): AudioParamLike;
  /**
   * Ramps exponentially from the previous event to `value`, arriving at
   * `time`. Both ends must be positive: WebAudio throws on a zero.
   */
  exponentialRampToValueAtTime(value: number, time: number): AudioParamLike;
  /** Approaches `target` from `startTime` with the time constant given. */
  setTargetAtTime(
    target: number,
    startTime: number,
    timeConstant: number,
  ): AudioParamLike;
  /** Drops every event at or after `time`. */
  cancelScheduledValues(time: number): AudioParamLike;
}

/** A node in the graph: it can feed another node or a parameter. */
export interface AudioNodeLike {
  /** Feeds this node's output into `destination`'s input. */
  connect(destination: AudioNodeLike): unknown;
  /** Feeds this node's output into a parameter (an LFO into a gain). */
  connect(destination: AudioParamLike): unknown;
  /** Cuts every outgoing connection. */
  disconnect(): void;
}

/**
 * A node that starts and stops on the context's clock, and says when it
 * ended. Both oscillators and buffer sources are one.
 */
export interface ScheduledSourceLike extends AudioNodeLike {
  /** Starts at `when` (context seconds). */
  start(when?: number): void;
  /** Stops at `when`; a later call replaces an earlier one. */
  stop(when?: number): void;
  /** Called once the source has stopped. */
  onended: ((ev: Event) => unknown) | null;
}

/** An oscillator: a built-in wave or a periodic wave. */
export interface OscillatorLike extends ScheduledSourceLike {
  type: OscillatorType;
  readonly frequency: AudioParamLike;
  readonly detune: AudioParamLike;
  /** Plays `wave` instead of a built-in type. */
  setPeriodicWave(wave: PeriodicWaveLike): void;
}

/** A buffer player, used for noise. */
export interface BufferSourceLike extends ScheduledSourceLike {
  buffer: AudioBufferLike | null;
  loop: boolean;
  readonly playbackRate: AudioParamLike;
}

/** A gain: the envelopes, the buses, the master, an inverter at -1. */
export interface GainLike extends AudioNodeLike {
  readonly gain: AudioParamLike;
}

/** A resonant filter. */
export interface BiquadLike extends AudioNodeLike {
  type: BiquadFilterType;
  readonly frequency: AudioParamLike;
  readonly Q: AudioParamLike;
  readonly gain: AudioParamLike;
}

/** A delay line; the pulse wave's width. */
export interface DelayLike extends AudioNodeLike {
  readonly delayTime: AudioParamLike;
}

/** A stereo panner: -1 left to 1 right. */
export interface PannerLike extends AudioNodeLike {
  readonly pan: AudioParamLike;
}

/** A block of samples. */
export interface AudioBufferLike {
  /** The samples of one channel, writable in place. */
  getChannelData(channel: number): Float32Array;
  readonly length: number;
  readonly sampleRate: number;
  readonly duration: number;
}

/**
 * A periodic wave as the context made it. Opaque: the synth only hands it
 * back to `setPeriodicWave`. It is `object` because the DOM's
 * `PeriodicWave` has no members for a brand to match.
 */
export type PeriodicWaveLike = object;

/** What a live and an offline context share: the clock and the factories. */
export interface BaseAudioContextLike {
  /** The context's clock in seconds; all scheduling reads it. */
  readonly currentTime: number;
  readonly sampleRate: number;
  readonly destination: AudioNodeLike;
  createOscillator(): OscillatorLike;
  createGain(): GainLike;
  createBiquadFilter(): BiquadLike;
  /** A delay line able to hold up to `maxDelayTime` seconds (default 1). */
  createDelay(maxDelayTime?: number): DelayLike;
  createStereoPanner(): PannerLike;
  createBufferSource(): BufferSourceLike;
  createBuffer(
    numberOfChannels: number,
    length: number,
    sampleRate: number,
  ): AudioBufferLike;
  /** A wave from its cosine (`real`) and sine (`imag`) partials. */
  createPeriodicWave(
    real: Float32Array,
    imag: Float32Array,
    constraints?: { disableNormalization?: boolean },
  ): PeriodicWaveLike;
}

/** A live context: the base plus its lifecycle. Only the mixer needs it. */
export interface AudioContextLike extends BaseAudioContextLike {
  readonly state: AudioContextState;
  resume(): Promise<void>;
  suspend(): Promise<void>;
  close(): Promise<void>;
}
