import { describe, expect, it } from "vitest";

import {
  CARRIER_QUIET_MS,
  DTMF,
  HANDSHAKE_MS,
  carrierGate,
  dialNumber,
  handshakePatch,
  hangupPatch,
  reconnectPatch,
} from "./modem";
import { patchLength, valueAt } from "./patch";

describe("dialNumber", () => {
  // Mutation caught: a number that is not the fictional 555-01 block, or
  // one that changes between calls.
  it("dials a fictional number, stable for a domain (M4 C26)", () => {
    expect(dialNumber("eng")).toMatch(/^555-01\d\d$/);
    expect(dialNumber("eng")).toBe(dialNumber("eng"));
    expect(dialNumber(null)).toMatch(/^555-01\d\d$/);
    expect(dialNumber(null)).toBe(dialNumber(null));
    // Two digits over many domains: not always the same number.
    const numbers = new Set(
      ["eng", "ops", "notes", "people", "design", "sales"].map(dialNumber),
    );
    expect(numbers.size).toBeGreaterThan(1);
  });
});

describe("handshakePatch", () => {
  // Mutation caught: the 4 to 6 s bound broken, the digits out of order,
  // the `-` dialled, or a digit's pair wrong.
  it("dials the number's digits, then answers and shakes hands in about HANDSHAKE_MS", () => {
    const number = "555-0142";
    const patch = handshakePatch(number);
    const length = patchLength(patch);
    expect(length).toBeGreaterThanOrEqual(4);
    expect(length).toBeLessThanOrEqual(6);
    expect(Math.abs(length * 1000 - HANDSHAKE_MS)).toBeLessThan(200);

    const digits = [..."5550142"];
    digits.forEach((digit, i) => {
      const pair = DTMF[digit];
      if (pair === undefined) throw new Error(`no tones for ${digit}`);
      const row = patch.voices[2 * i];
      const column = patch.voices[2 * i + 1];
      expect(row?.wave, digit).toBe("sine");
      expect(column?.wave, digit).toBe("sine");
      expect(valueAt(row?.pitch ?? [], row?.at ?? 0), digit).toBe(pair[0]);
      expect(valueAt(column?.pitch ?? [], column?.at ?? 0), digit).toBe(
        pair[1],
      );
      expect(row?.gain).toBe(0.35);
      // One after the other, never together.
      if (i > 0) {
        const previous = patch.voices[2 * i - 2];
        expect(row?.at ?? 0).toBeGreaterThan(previous?.at ?? 0);
      }
    });
    // After the last digit comes the answer tone, not another digit.
    const next = patch.voices[2 * digits.length];
    expect(valueAt(next?.pitch ?? [], next?.at ?? 0)).toBe(2100);
  });

  it("names the keypad's twelve keys", () => {
    expect(Object.keys(DTMF).sort()).toEqual([..."0123456789*#"].sort());
  });
});

describe("reconnect and hang-up", () => {
  // Mutation caught: the reconnect as long as the launch's handshake, or a
  // hang-up that rings on.
  it("keeps the reconnect near a second and the hang-up a click", () => {
    const reconnect = patchLength(reconnectPatch());
    expect(reconnect).toBeGreaterThan(0.8);
    expect(reconnect).toBeLessThan(1.3);
    expect(patchLength(hangupPatch())).toBeLessThan(0.1);
  });
});

describe("carrierGate", () => {
  // Mutation caught: the limit on the down only (an up without its down
  // plays), or the quiet time measured from the last change of any kind.
  it("rate limits NO CARRIER to once a minute (Review Focus 4)", () => {
    let state: { downAt: number | null; lastAt: number } = {
      downAt: null,
      lastAt: -Infinity,
    };
    const step = (up: boolean, now: number) => {
      const r = carrierGate(state, up, now);
      state = r.state;
      return r.play;
    };
    expect(step(true, 0), "an up with no down before it").toBe(false);
    expect(step(false, 0), "the first down").toBe(true);
    expect(step(true, 5_000), "its up").toBe(true);
    expect(step(false, 30_000), "a down inside the quiet time").toBe(false);
    expect(step(true, 35_000), "and its up").toBe(false);
    expect(step(false, 61_000), "a down after the quiet time").toBe(true);
    expect(step(true, 62_000)).toBe(true);
    expect(CARRIER_QUIET_MS).toBe(60_000);
  });
});
