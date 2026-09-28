/**
 * A recording stand-in for a live audio context, for tests only.
 *
 * jsdom has no WebAudio, and even a real context would not say what it was
 * asked to do. `FakeAudioContext` implements `AudioContextLike` and keeps a
 * record instead: every node it made (in `nodes`, in creation order), each
 * node's parameters with every event scheduled on them (`[method, value,
 * time]`), its outgoing connections, and the times it was started and
 * stopped. Tests play a patch or build a mixer on it and read the schedule
 * back.
 *
 * It enforces the two WebAudio rules a schedule most easily breaks: an
 * exponential ramp to a value that is not positive throws a `RangeError`, and
 * a delay line cannot be made longer than 180 s or shorter than nothing.
 * Nothing in the app imports this file.
 */

import type {
  AudioBufferLike,
  AudioContextLike,
  AudioNodeLike,
  AudioParamLike,
  BiquadLike,
  BufferSourceLike,
  DelayLike,
  GainLike,
  OscillatorLike,
  PannerLike,
  PeriodicWaveLike,
} from "./context";

/** The scheduling methods a parameter records. */
export type ParamMethod =
  | "setValueAtTime"
  | "linearRampToValueAtTime"
  | "exponentialRampToValueAtTime"
  | "setTargetAtTime"
  | "cancelScheduledValues";

/**
 * One scheduled event. `setTargetAtTime` records its target and start time
 * (the time constant is dropped); `cancelScheduledValues` records a value
 * of `NaN`.
 */
export type ParamEvent = [method: ParamMethod, value: number, time: number];

/** A parameter that records its events and keeps `value` as last assigned. */
export class FakeParam implements AudioParamLike {
  value: number;
  readonly events: ParamEvent[] = [];

  constructor(value: number) {
    this.value = value;
  }

  setValueAtTime(value: number, time: number): this {
    this.events.push(["setValueAtTime", value, time]);
    return this;
  }

  linearRampToValueAtTime(value: number, time: number): this {
    this.events.push(["linearRampToValueAtTime", value, time]);
    return this;
  }

  exponentialRampToValueAtTime(value: number, time: number): this {
    if (!(value > 0)) {
      throw new RangeError(`exponential ramp to ${value}, not positive`);
    }
    this.events.push(["exponentialRampToValueAtTime", value, time]);
    return this;
  }

  setTargetAtTime(target: number, startTime: number): this {
    this.events.push(["setTargetAtTime", target, startTime]);
    return this;
  }

  cancelScheduledValues(time: number): this {
    this.events.push(["cancelScheduledValues", Number.NaN, time]);
    return this;
  }
}

/** The kinds of node the fake makes. */
export type FakeKind =
  | "destination"
  | "oscillator"
  | "gain"
  | "biquad"
  | "delay"
  | "panner"
  | "bufferSource";

/** Any node the fake made: its kind, parameters, connections and times. */
export class FakeNode implements AudioNodeLike {
  readonly kind: FakeKind;
  /** The node's parameters by their WebAudio name (`gain`, `frequency`, ...). */
  readonly params: Record<string, FakeParam>;
  /** Everything this node feeds, in the order connected. */
  readonly connections: (FakeNode | FakeParam)[] = [];
  /** Every `start` time, in call order. */
  readonly started: number[] = [];
  /** Every `stop` time, in call order; the last one is the one that holds. */
  readonly stopped: number[] = [];
  /** How often `disconnect()` was called. */
  disconnects = 0;

  constructor(kind: FakeKind, params: Record<string, FakeParam> = {}) {
    this.kind = kind;
    this.params = params;
  }

  connect(destination: AudioNodeLike | AudioParamLike): unknown {
    if (!(
      destination instanceof FakeNode || destination instanceof FakeParam
    )) {
      throw new TypeError("connected to something the fake did not make");
    }
    this.connections.push(destination);
    return destination;
  }

  disconnect(): void {
    this.disconnects += 1;
    this.connections.length = 0;
  }
}

/** A fake start/stop source's shared part. */
class FakeSource extends FakeNode {
  onended: ((ev: Event) => unknown) | null = null;

  start(when = 0): void {
    this.started.push(when);
  }

  stop(when = 0): void {
    if (this.started.length === 0) {
      throw new Error("InvalidStateError: stop before start");
    }
    this.stopped.push(when);
  }

  /** Fires `onended` as the browser would once the source stopped. */
  end(): void {
    this.onended?.(new Event("ended"));
  }
}

/** A fake oscillator; remembers its type and any periodic wave. */
export class FakeOscillator extends FakeSource implements OscillatorLike {
  type: OscillatorType = "sine";
  wave: PeriodicWaveLike | null = null;
  readonly frequency = new FakeParam(440);
  readonly detune = new FakeParam(0);

  constructor() {
    super("oscillator");
    this.params.frequency = this.frequency;
    this.params.detune = this.detune;
  }

  setPeriodicWave(wave: PeriodicWaveLike): void {
    this.type = "custom";
    this.wave = wave;
  }
}

/** A fake buffer player. */
export class FakeBufferSource extends FakeSource implements BufferSourceLike {
  buffer: AudioBufferLike | null = null;
  loop = false;
  readonly playbackRate = new FakeParam(1);

  constructor() {
    super("bufferSource");
    this.params.playbackRate = this.playbackRate;
  }
}

/** A fake gain. */
export class FakeGain extends FakeNode implements GainLike {
  readonly gain = new FakeParam(1);

  constructor() {
    super("gain");
    this.params.gain = this.gain;
  }
}

/** A fake filter. */
export class FakeBiquad extends FakeNode implements BiquadLike {
  type: BiquadFilterType = "lowpass";
  readonly frequency = new FakeParam(350);
  readonly Q = new FakeParam(1);
  readonly gain = new FakeParam(0);

  constructor() {
    super("biquad");
    this.params.frequency = this.frequency;
    this.params.Q = this.Q;
    this.params.gain = this.gain;
  }
}

/** A fake delay line; remembers the longest delay it was made for. */
export class FakeDelay extends FakeNode implements DelayLike {
  readonly delayTime = new FakeParam(0);
  readonly maxDelayTime: number;

  constructor(maxDelayTime: number) {
    super("delay");
    this.maxDelayTime = maxDelayTime;
    this.params.delayTime = this.delayTime;
  }
}

/** A fake panner. */
export class FakePanner extends FakeNode implements PannerLike {
  readonly pan = new FakeParam(0);

  constructor() {
    super("panner");
    this.params.pan = this.pan;
  }
}

/** A fake buffer holding real samples. */
export class FakeBuffer implements AudioBufferLike {
  readonly length: number;
  readonly sampleRate: number;
  private readonly channels: Float32Array[];

  constructor(channels: number, length: number, sampleRate: number) {
    this.length = length;
    this.sampleRate = sampleRate;
    this.channels = Array.from(
      { length: channels },
      () => new Float32Array(length),
    );
  }

  get duration(): number {
    return this.length / this.sampleRate;
  }

  getChannelData(channel: number): Float32Array {
    const data = this.channels[channel];
    if (data === undefined) throw new RangeError(`no channel ${channel}`);
    return data;
  }
}

/** A periodic wave as the fake made it: its partials, kept for assertions. */
export interface FakeWave {
  readonly real: readonly number[];
  readonly imag: readonly number[];
}

/** The recording context. */
export class FakeAudioContext implements AudioContextLike {
  /** Settable: tests move the clock by hand. */
  currentTime = 0;
  readonly sampleRate: number;
  /** Settable: tests start a context running or suspended. */
  state: AudioContextState = "suspended";
  readonly destination = new FakeNode("destination");
  /** Every node made, in creation order (the destination is not listed). */
  readonly nodes: FakeNode[] = [];
  /** Every buffer made, in creation order. */
  readonly buffers: FakeBuffer[] = [];
  /** `resume`, `suspend` and `close`, in call order. */
  readonly calls: ("resume" | "suspend" | "close")[] = [];
  /**
   * When true, `resume` and `suspend` leave `state` as it is until
   * `settle()`, as a real context does while its promise is pending.
   */
  deferred = false;
  private pending: AudioContextState | null = null;

  constructor(sampleRate = 48000) {
    this.sampleRate = sampleRate;
  }

  private made<T extends FakeNode>(node: T): T {
    this.nodes.push(node);
    return node;
  }

  /** The nodes of one kind, in creation order. */
  ofKind(kind: "oscillator"): FakeOscillator[];
  ofKind(kind: "gain"): FakeGain[];
  ofKind(kind: "biquad"): FakeBiquad[];
  ofKind(kind: "delay"): FakeDelay[];
  ofKind(kind: "bufferSource"): FakeBufferSource[];
  ofKind(kind: FakeKind): FakeNode[];
  ofKind(kind: FakeKind): FakeNode[] {
    return this.nodes.filter((node) => node.kind === kind);
  }

  createOscillator(): FakeOscillator {
    return this.made(new FakeOscillator());
  }

  createGain(): FakeGain {
    return this.made(new FakeGain());
  }

  createBiquadFilter(): FakeBiquad {
    return this.made(new FakeBiquad());
  }

  createDelay(maxDelayTime = 1): FakeDelay {
    if (!(maxDelayTime > 0 && maxDelayTime < 180)) {
      throw new Error(`NotSupportedError: maxDelayTime ${maxDelayTime}`);
    }
    return this.made(new FakeDelay(maxDelayTime));
  }

  createStereoPanner(): FakePanner {
    return this.made(new FakePanner());
  }

  createBufferSource(): FakeBufferSource {
    return this.made(new FakeBufferSource());
  }

  createBuffer(
    numberOfChannels: number,
    length: number,
    sampleRate: number,
  ): FakeBuffer {
    const buffer = new FakeBuffer(numberOfChannels, length, sampleRate);
    this.buffers.push(buffer);
    return buffer;
  }

  createPeriodicWave(real: Float32Array, imag: Float32Array): FakeWave {
    return { real: Array.from(real), imag: Array.from(imag) };
  }

  resume(): Promise<void> {
    this.calls.push("resume");
    if (this.state === "closed") {
      return Promise.reject(new Error("InvalidStateError: closed"));
    }
    this.change("running");
    return Promise.resolve();
  }

  suspend(): Promise<void> {
    this.calls.push("suspend");
    if (this.state === "closed") {
      return Promise.reject(new Error("InvalidStateError: closed"));
    }
    this.change("suspended");
    return Promise.resolve();
  }

  /** Applies the state the last deferred `resume` or `suspend` asked for. */
  settle(): void {
    if (this.pending !== null && this.state !== "closed") {
      this.state = this.pending;
    }
    this.pending = null;
  }

  private change(next: AudioContextState): void {
    if (this.deferred) this.pending = next;
    else this.state = next;
  }

  close(): Promise<void> {
    this.calls.push("close");
    this.state = "closed";
    return Promise.resolve();
  }
}
