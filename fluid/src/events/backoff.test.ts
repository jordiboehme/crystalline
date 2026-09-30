import { describe, expect, it } from "vitest";

import { RECONNECT_MAX_MS, reconnectDelay } from "./backoff";

describe("the reconnect backoff", () => {
  it("doubles from one second: 1, 2, 4, 8, 16 seconds at the middle of the jitter", () => {
    const middle = () => 0.5;
    expect([0, 1, 2, 3, 4].map((n) => reconnectDelay(n, middle))).toEqual([
      1_000, 2_000, 4_000, 8_000, 16_000,
    ]);
  });

  it("spreads each wait by a fifth either way", () => {
    expect(reconnectDelay(1, () => 0)).toBe(1_600);
    expect(reconnectDelay(1, () => 1)).toBe(2_400);
  });

  it("never waits longer than thirty seconds, jitter included", () => {
    // Catches a cap applied before the jitter, or not at all.
    for (const attempt of [5, 6, 20, 1_000]) {
      expect(reconnectDelay(attempt, () => 1)).toBe(RECONNECT_MAX_MS);
    }
    expect(RECONNECT_MAX_MS).toBe(30_000);
  });
});
