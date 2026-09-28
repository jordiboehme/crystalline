/**
 * The modem (M4 C26, C27): the station dials in when it starts, and hangs
 * up when Fluid's change stream drops.
 *
 * - **The dial-in.** A launch from the C64 screen dials a fictional number
 *   in the 555-01 block (`dialNumber`, two digits from the domain's seed),
 *   then the far end answers and the two shake hands: the keypad's tones
 *   for each digit, the answer tone with its phase-reversal clicks, the
 *   alternating bursts and the screech, `HANDSHAKE_MS` in all
 *   (`handshakePatch`).
 * - **NO CARRIER.** A drop of the stream plays the hang-up click
 *   (`hangupPatch`) and its return about a second of handshake
 *   (`reconnectPatch`), at most once per `CARRIER_QUIET_MS` for the pair
 *   (`carrierGate`).
 *
 * Every function answers a `Patch` (`audio/patch.ts`) the director plays
 * on the `modem` bus; nothing here touches WebAudio. As everywhere in a
 * patch, every step's `at` runs from the patch start.
 */

import { seedFor } from "../core/seed";
import type { Patch, Step, Voice } from "./patch";

/** The keypad's row and column tones. */
export const DTMF: Record<string, readonly [number, number]> = {
  "1": [697, 1209],
  "2": [697, 1336],
  "3": [697, 1477],
  "4": [770, 1209],
  "5": [770, 1336],
  "6": [770, 1477],
  "7": [852, 1209],
  "8": [852, 1336],
  "9": [852, 1477],
  "*": [941, 1209],
  "0": [941, 1336],
  "#": [941, 1477],
};

/** How long the launch's dial-in lasts, in milliseconds (M4 C26). */
export const HANDSHAKE_MS = 5000;

/** The quiet time after a NO CARRIER that played (M4 C27). */
export const CARRIER_QUIET_MS = 60_000;

/** Where the tab remembers that the station dialled in (M4 C26). */
export const CONNECTED_KEY = "station.connected";

/** A key's tones: on, then off before the next key. */
const KEY_ON_S = 0.09;
const KEY_OFF_S = 0.07;
const KEY_GAIN = 0.35;

/** The answer tone and the gap its phase reversals make. */
const ANSWER_HZ = 2100;
const ANSWER_S = 1.0;
const REVERSAL_S = 0.45;
const DIP_S = 0.002;
const ANSWER_GAIN = 0.3;

/** The alternating bursts: their tones, their length, their count. */
const BURSTS = [1200, 2400] as const;
const BURST_S = 0.15;
const BURST_PAIRS = 4;
const BURST_GAIN = 0.28;

/** The screech: its band and the pulse's arpeggio under it. */
const SCREECH_S = 1.6;
const SCREECH_BAND = { hz: 1800, q: 1.5 } as const;
const SCREECH_ARP = [1650, 1850] as const;
const SCREECH_STEP_S = 0.02;
const SCREECH_NOISE_GAIN = 0.9;
const SCREECH_PULSE_GAIN = 0.14;

/** The fade at the end of a handshake. */
const FADE_S = 0.04;

/** The fictional number the launch dials for a domain (M4 C26). */
export function dialNumber(domain: string | null): string {
  // The airlock (no domain) has a seed of its own, which no domain's
  // name can share.
  const seed = domain === null ? seedFor("dial") : seedFor("dial", domain);
  return `555-01${String(seed % 100).padStart(2, "0")}`;
}

/** A steady tone from `at`, gated for `length` with a quick edge. */
function tone(
  hz: number,
  at: number,
  length: number,
  gain: number,
  edge = 0.005,
): Voice {
  return {
    wave: "sine",
    pitch: [{ at, value: hz }],
    env: { a: edge, d: 0.01, s: 1, r: edge },
    length: length - edge,
    at,
    gain,
  };
}

/**
 * The answer tone from `at` for `length`, dipping for `DIP_S` every
 * `REVERSAL_S`: the click a phase reversal makes. Split into one voice per
 * stretch, since a patch has no gain automation of its own.
 */
function answerTone(at: number, length: number): Voice[] {
  const voices: Voice[] = [];
  for (let from = 0; from < length - 1e-9; from += REVERSAL_S) {
    const last = from + REVERSAL_S >= length - 1e-9;
    const span = last ? length - from : REVERSAL_S - DIP_S;
    const edge = last ? FADE_S / 4 : DIP_S / 2;
    voices.push(tone(ANSWER_HZ, at + from, span, ANSWER_GAIN, edge));
  }
  return voices;
}

/**
 * The screech from `at` for `length`: noise through a bandpass at 1800 Hz
 * and a pulse stepping between 1650 and 1850 Hz under it, fading out over
 * `FADE_S`.
 */
function screech(at: number, length: number): Voice[] {
  const env = { a: 0.01, d: 0.05, s: 1, r: FADE_S };
  const count = Math.max(1, Math.round(length / SCREECH_STEP_S));
  const arp: Step[] = Array.from({ length: count }, (_, i) => ({
    at: at + i * SCREECH_STEP_S,
    value: SCREECH_ARP[i % SCREECH_ARP.length] as number,
  }));
  return [
    {
      wave: "noise",
      pitch: [],
      filter: {
        type: "bandpass",
        cutoff: [{ at, value: SCREECH_BAND.hz }],
        q: SCREECH_BAND.q,
      },
      env,
      length: length - FADE_S,
      at,
      gain: SCREECH_NOISE_GAIN,
    },
    {
      wave: "pulse",
      pitch: arp,
      width: [{ at, value: 0.5 }],
      env,
      length: length - FADE_S,
      at,
      gain: SCREECH_PULSE_GAIN,
    },
  ];
}

/**
 * Dial tones for the number's digits (the `-` skipped), the answer tone
 * and the handshake, `HANDSHAKE_MS` in all. The keypad's pairs come
 * first in `voices`, row then column, in the order they are dialled.
 */
export function handshakePatch(number: string): Patch {
  const digits = [...number].filter((c) => DTMF[c] !== undefined);
  const keys = digits.flatMap((digit, i): Voice[] => {
    const [row, column] = DTMF[digit] as readonly [number, number];
    const at = i * (KEY_ON_S + KEY_OFF_S);
    return [
      tone(row, at, KEY_ON_S, KEY_GAIN),
      tone(column, at, KEY_ON_S, KEY_GAIN),
    ];
  });
  const answerAt = digits.length * (KEY_ON_S + KEY_OFF_S);
  const burstsAt = answerAt + ANSWER_S;
  const burstsS = BURST_S * BURSTS.length * BURST_PAIRS;
  const screechAt = burstsAt + burstsS;
  const bursts: Voice = {
    wave: "sine",
    pitch: Array.from({ length: BURSTS.length * BURST_PAIRS }, (_, i) => ({
      at: burstsAt + i * BURST_S,
      value: BURSTS[i % BURSTS.length] as number,
    })),
    env: { a: 0.005, d: 0.01, s: 1, r: 0.005 },
    length: burstsS - 0.005,
    at: burstsAt,
    gain: BURST_GAIN,
  };
  return {
    name: "modem handshake",
    voices: [
      ...keys,
      ...answerTone(answerAt, ANSWER_S),
      bursts,
      ...screech(screechAt, SCREECH_S),
    ],
  };
}

/** About one second of handshake, on reconnect (M4 C27). */
export function reconnectPatch(): Patch {
  const answer = 0.3;
  return {
    name: "modem reconnect",
    voices: [...answerTone(0, answer), ...screech(answer, 0.7)],
  };
}

/** The hang-up click: a short noise click and a low thump (M4 C27). */
export function hangupPatch(): Patch {
  return {
    name: "modem hang-up",
    voices: [
      {
        wave: "noise",
        pitch: [],
        filter: { type: "highpass", cutoff: [{ at: 0, value: 2000 }], q: 0.7 },
        env: { a: 0.001, d: 0.004, s: 0.5, r: 0.004 },
        length: 0.008,
        at: 0,
        gain: 0.35,
      },
      {
        wave: "sine",
        pitch: [
          { at: 0, value: 90 },
          { at: 0.03, value: 60, ramp: "exp" },
        ],
        env: { a: 0.002, d: 0.01, s: 0.7, r: 0.01 },
        length: 0.02,
        at: 0,
        gain: 0.45,
      },
    ],
  };
}

/** The route's memory of the carrier's sounds, for `carrierGate`. */
export interface CarrierState {
  /** When the down that played was heard; null once its up played, or when it did not play. */
  downAt: number | null;
  /** When the last down that played was heard (`-Infinity` before any). */
  lastAt: number;
}

/** The gate's state before any change. */
export const CARRIER_START: CarrierState = { downAt: null, lastAt: -Infinity };

/**
 * The rate limit of C27: answers whether this change may sound, and the
 * new state. A down plays when no down played in the last
 * `CARRIER_QUIET_MS`; an up plays only after a down that played, so the
 * pair sounds together or not at all.
 */
export function carrierGate(
  state: CarrierState,
  up: boolean,
  now: number,
): { play: boolean; state: CarrierState } {
  if (up) {
    return {
      play: state.downAt !== null,
      state: { downAt: null, lastAt: state.lastAt },
    };
  }
  if (now - state.lastAt >= CARRIER_QUIET_MS) {
    return { play: true, state: { downAt: now, lastAt: now } };
  }
  return { play: false, state: { downAt: null, lastAt: state.lastAt } };
}
