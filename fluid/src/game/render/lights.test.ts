import { describe, expect, it } from "vitest";

import type { LightSpecial, LightZone } from "../world/types";
import {
  FASTDARK,
  FLICKER_TICS,
  GLOWSPEED,
  STROBEBRIGHT,
  createLights,
  minLevelOf,
} from "./lights";

function zone(special: LightSpecial, level = 200, seed = 1): LightZone {
  return { x0: 0, y0: 0, x1: 1, y1: 1, level, special, seed };
}

function trace(z: LightZone, ticks: number) {
  const lights = createLights([z]);
  const out: number[] = [];
  for (let i = 0; i < ticks; i++) {
    lights.tick();
    out.push(lights.levels[0] ?? NaN);
  }
  return out;
}

describe("light specials", () => {
  it("keeps a steady light steady", () => {
    expect(new Set(trace(zone("steady"), 100))).toEqual(new Set([200]));
  });

  it("glows by GLOWSPEED per tick between its bounds and turns around", () => {
    const z = zone("glow");
    const levels = trace(z, 200);
    const min = minLevelOf(z);
    for (let i = 1; i < levels.length; i++) {
      const step = Math.abs((levels[i] ?? 0) - (levels[i - 1] ?? 0));
      expect(step).toBeLessThanOrEqual(GLOWSPEED);
    }
    expect(Math.min(...levels)).toBeLessThanOrEqual(min + GLOWSPEED);
    expect(Math.max(...levels)).toBe(200);
  });

  it("strobes bright for STROBEBRIGHT ticks and dark for FASTDARK", () => {
    const z = zone("strobe");
    const levels = trace(z, 200);
    // Run lengths after the first (seeded) phase.
    const runs: { bright: boolean; n: number }[] = [];
    for (const l of levels) {
      const bright = l === 200;
      const last = runs[runs.length - 1];
      if (last !== undefined && last.bright === bright) last.n++;
      else runs.push({ bright, n: 1 });
    }
    for (const r of runs.slice(1, -1)) {
      expect(r.n).toBe(r.bright ? STROBEBRIGHT : FASTDARK);
    }
  });

  it("flickers only every FLICKER_TICS ticks and stays inside its bounds", () => {
    const z = zone("flicker");
    const levels = trace(z, 400);
    const min = minLevelOf(z);
    for (const l of levels) {
      expect(l).toBeGreaterThanOrEqual(min);
      expect(l).toBeLessThanOrEqual(200);
    }
    let changes = 0;
    for (let i = 1; i < levels.length; i++)
      if (levels[i] !== levels[i - 1]) changes++;
    expect(changes).toBeLessThanOrEqual(Math.ceil(400 / FLICKER_TICS));
    expect(changes).toBeGreaterThan(0);
  });

  it("fails between exactly two levels", () => {
    const z = zone("failing");
    const values = new Set(trace(z, 1000));
    expect(values).toEqual(new Set([200, minLevelOf(z)]));
  });

  it("replays the same flicker from the same seed and differs between seeds", () => {
    expect(trace(zone("flicker", 200, 5), 100)).toEqual(
      trace(zone("flicker", 200, 5), 100),
    );
    expect(trace(zone("flicker", 200, 5), 100)).not.toEqual(
      trace(zone("flicker", 200, 6), 100),
    );
  });

  it("starts every zone at its level before the first tick", () => {
    const lights = createLights([zone("steady", 180), zone("glow", 90)]);
    expect(Array.from(lights.levels)).toEqual([180, 90]);
  });
});
