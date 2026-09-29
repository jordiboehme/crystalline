/**
 * Sounds as data: the patch format of the station's synth.
 *
 * The synth is SID-style, after the sound chip of the eight-bit home
 * computers: a few voices, each an oscillator with a volume envelope, and
 * the three tricks that make that chip recognisable, all expressible here:
 *
 * - **Pulse width modulation.** A `pulse` voice takes a `width` step list
 *   (0.05 to 0.95) that moves over time, the thin nasal sweep of a pulse
 *   whose duty cycle changes while it plays.
 * - **A resonant filter swept over time.** A voice's `filter` has a cutoff
 *   step list and a resonance (`q`), for the wah of a door hiss or a lift
 *   hum rising.
 * - **50 Hz arpeggios.** `arpeggio` steps a pitch through a chord every 20
 *   ms (the chip's usual frame rate), which the ear hears as one warbling
 *   chord.
 *
 * Why data rather than code that pokes WebAudio: a patch can be read and
 * asserted without any audio (the tests check lengths and pitches), the
 * dev sound board lists every patch by name and plays it on a click, and an
 * offline context renders the same data to a buffer for an ear check and a
 * length measurement. `audio/synth.ts` is the one place that turns a patch
 * into nodes.
 *
 * Times: every `Step.at` is in seconds from the patch's start (the `when`
 * the synth is given), not from its voice's `at`. A voice's envelope, its
 * sources' start and its gate run from `when + voice.at`. So a voice that
 * starts late and wants its pitch to move with it writes its steps from
 * the patch start (an `arpeggio` for a voice at 0.5 s is shifted by 0.5).
 */

/**
 * A value over time, relative to the patch start, in seconds. Without a
 * `ramp` the value jumps at `at`; with one it is reached at `at`, ramping
 * from the step before (linearly, or exponentially for pitches and
 * cutoffs, which the ear hears on a log scale).
 */
export type Step = { at: number; value: number; ramp?: "linear" | "exp" };

/**
 * Attack, decay, sustain level, release, in seconds and 0..1. The level
 * rises from 0 to the voice's gain over `a`, falls to `gain * s` over `d`,
 * holds until the gate ends (`length`), then falls to 0 over `r`.
 */
export interface Env {
  a: number;
  d: number;
  s: number;
  r: number;
}

/**
 * A voice's oscillator. `pulse` is the width-modulated pulse; `saw`,
 * `triangle` and `sine` are the built-in waves; `noise` a looping seeded
 * noise buffer; `organ` a few harmonics of a drawbar organ.
 */
export type Wave = "pulse" | "saw" | "triangle" | "sine" | "noise" | "organ";

/** One voice of a patch. */
export interface Voice {
  wave: Wave;
  /** Hz steps; ignored for noise. */
  pitch: Step[];
  /** Pulse width 0.05..0.95 steps (pulse only): the width modulation. */
  width?: Step[];
  /** Resonant filter: type, cutoff steps in Hz, Q. */
  filter?: {
    type: "lowpass" | "bandpass" | "highpass";
    cutoff: Step[];
    q: number;
  };
  env: Env;
  /** Gate length in seconds: the release starts here. */
  length: number;
  /** Start offset within the patch, seconds. */
  at: number;
  gain: number;
  /** Tremolo: rate in Hz and depth 0..1. */
  tremolo?: { rate: number; depth: number };
  /**
   * Pitch drift: a slow sine on the pitch, rate in Hz and depth in cents,
   * for a drone that wavers. Every oscillator of the voice (both saws of a
   * pulse) drifts together.
   */
  drift?: { rate: number; cents: number };
  /**
   * Pulse width modulation by a sine (pulse only): rate in Hz and depth in
   * width, swinging the width's steps up and down by `depth`. Unlike a
   * `width` step list it keeps moving in a looping patch. The steps plus
   * and minus `depth` should stay inside 0.05..0.95.
   */
  pwm?: { rate: number; depth: number };
}

/**
 * A named sound. `loop`: the voices hold at their last step until stopped
 * (drones, the lift's held hum); their envelopes stay at the sustain level
 * and no source is stopped until the caller stops the patch.
 */
export interface Patch {
  name: string;
  voices: Voice[];
  loop?: boolean;
}

/** The pitch of a MIDI note number, equal-tempered, note 69 at 440 Hz. */
export function midiHz(note: number): number {
  return 440 * 2 ** ((note - 69) / 12);
}

/** The step length of a 50 Hz arpeggio. */
export const ARPEGGIO_STEP_S = 0.02;

/**
 * A 50 Hz arpeggio: the notes (MIDI numbers) cycled every `stepS` over
 * `length`, as held pitch steps from 0. The count is rounded so float
 * error in `length / stepS` never adds or drops a step.
 */
export function arpeggio(
  notes: readonly number[],
  length: number,
  stepS = ARPEGGIO_STEP_S,
): Step[] {
  if (notes.length === 0 || !(stepS > 0)) return [];
  const count = Math.max(1, Math.round(length / stepS));
  return Array.from({ length: count }, (_, i) => ({
    at: i * stepS,
    value: midiHz(notes[i % notes.length] as number),
  }));
}

/**
 * When the last voice's release ends, in seconds from the patch start (a
 * looping patch: its first cycle). Zero for a patch with no voices.
 */
export function patchLength(patch: Patch): number {
  return patch.voices.reduce(
    (end, v) => Math.max(end, v.at + v.length + v.env.r),
    0,
  );
}

/**
 * The smallest target an exponential step reaches. WebAudio throws on an
 * exponential ramp to 0, so the synth floors every `exp` step's value here,
 * and `valueAt` reads it the same way.
 */
export const MIN_EXP = 1e-4;

/** The value a step arrives at as the synth schedules it. */
function arrival(step: Step): number {
  return step.ramp === "exp" ? Math.max(step.value, MIN_EXP) : step.value;
}

/**
 * A step list's value at `t` (seconds from the patch start), the way
 * WebAudio plays the synth's schedule of it: the first value before the
 * first step (the synth holds it from the patch start), the ramps
 * interpolated, a plain step held until the next, the last value held
 * after the end. An `exp` step arrives at its value floored at `MIN_EXP`,
 * and an exponential ramp from a value that is not positive holds that
 * value until the step, as WebAudio does. Zero for an empty list. The
 * synth reads it to schedule the pulse's delay and level from lists whose
 * steps fall at different times.
 */
export function valueAt(steps: readonly Step[], t: number): number {
  const first = steps[0];
  if (first === undefined) return 0;
  if (t < first.at) return first.value;
  let prev = first;
  let from = first.value;
  for (let i = 1; i < steps.length; i++) {
    const next = steps[i] as Step;
    if (t < next.at) {
      const span = next.at - prev.at;
      if (next.ramp === undefined || span <= 0) return from;
      const to = arrival(next);
      const f = (t - prev.at) / span;
      if (next.ramp === "exp") {
        return from > 0 ? from * (to / from) ** f : from;
      }
      return from + (to - from) * f;
    }
    prev = next;
    from = arrival(next);
  }
  return from;
}
