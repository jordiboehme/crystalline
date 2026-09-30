/**
 * The gear curios' shape tests: what `curioModels.test.ts` does not check
 * for every kind. The light sword keeps its seven grip ribs on the lower
 * half of the hilt and its red button on the clamp in every variant,
 * lying or upright; lying in its cradle nothing rises over the prongs;
 * upright, the blade stands on the emitter at least 0.85 m long and the
 * blue and the green sword differ only in the blade's tint. The pistol's
 * glowing chamber sits on the body's top and is its highest part. The
 * gadget's top bulb stands off its base's axis, and the cluster stands on
 * three bases. The meter carries exactly seven wing lights, each on its
 * wing, whose groups make the chase visit them in the jumping order.
 */

import { describe, expect, it } from "vitest";

import { createBuilder, FLAG, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOKS } from "../../looks";
import {
  recordingKitAt,
  shape,
  toLocal,
  touching,
  type Part,
} from "../../modelChecks";
import { buildCurio, buildCurioMesh } from ".";
import { curioHalf } from "./common";
import { GADGET, HILT, METER, PISTOL, SWORD } from "./gear";

/** A curio's recorded parts, built at the origin. */
function partsOf(
  kind: "light-sword" | "green-pistol" | "pink-gadget" | "wing-meter",
  variant: number,
): Part[] {
  const parts: Part[] = [];
  buildCurio(
    recordingKitAt(createBuilder(), parts),
    kind,
    variant,
    LOOKS.aperture,
  );
  return parts;
}

/** A part's points in the recipe's local `[a, d, h]`. */
const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/** The bounds of a part in local `[a, d, h]`. */
function bounds(p: Part): { lo: V3; hi: V3 } {
  const ps = local(p);
  const lo = (k: 0 | 1 | 2) => Math.min(...ps.map((q) => q[k]));
  const hi = (k: 0 | 1 | 2) => Math.max(...ps.map((q) => q[k]));
  return { lo: [lo(0), lo(1), lo(2)], hi: [hi(0), hi(1), hi(2)] };
}

/** The middle of a part's local bounds. */
function centre(p: Part): V3 {
  const { lo, hi } = bounds(p);
  return [(lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2, (lo[2] + hi[2]) / 2];
}

/** Whether a part is a light in one of the eight blink groups. */
const blinks = (p: Part) => p.flag >= FLAG.blink && p.flag < FLAG.blink + 8;

/** Whether two parts touch, as the float check reads it. */
const meets = (p: Part, q: Part) => touching(shape(p.points), shape(q.points));

/**
 * The hilt's axis for a sword variant: `0` (`a`, lying in the cradle) or
 * `2` (`h`, upright in the stand), and where along it a distance `e` from
 * the emitter end lies.
 */
function axisOf(variant: number): { k: 0 | 2; at: (e: number) => number } {
  if (variant === 0) return { k: 0, at: (e) => HILT.length / 2 - e };
  return { k: 2, at: (e) => SWORD.standBase + HILT.length - e };
}

describe("gear curio models", () => {
  it("keeps seven ribs on the hilt's lower half and the button on its clamp, in every variant", () => {
    for (let v = 0; v < 3; v++) {
      const parts = partsOf("light-sword", v);
      const { k, at } = axisOf(v);
      // A rib is long along the hilt's axis and thin across it.
      const ribs = parts.filter((p) => {
        const { lo, hi } = bounds(p);
        const across = ([0, 1, 2] as const).filter((j) => j !== k);
        return (
          p.flag === FLAG.lit &&
          Math.abs(hi[k] - lo[k] - HILT.rib.length) < 1e-6 &&
          across.every((j) => hi[j] - lo[j] < 0.012)
        );
      });
      expect(ribs, `variant ${String(v)}`).toHaveLength(7);
      // The lower half is the half away from the emitter end.
      const [h0, h1] = [at(HILT.length / 2), at(HILT.length)].sort(
        (x, y) => x - y,
      );
      for (const r of ribs) {
        const { lo, hi } = bounds(r);
        expect(lo[k]).toBeGreaterThanOrEqual((h0 ?? 0) - 1e-6);
        expect(hi[k]).toBeLessThanOrEqual((h1 ?? 0) + 1e-6);
      }
      // The clamp is the lit box spanning the clamp's stretch of the axis;
      // the button is a small part in that stretch that stands proud of
      // it and touches it.
      const [c0, c1] = [at(HILT.clamp[1]), at(HILT.clamp[0])];
      const clamp = parts.filter((p) => {
        const { lo, hi } = bounds(p);
        return (
          p.method === "box" &&
          Math.abs(lo[k] - c0) < 1e-6 &&
          Math.abs(hi[k] - c1) < 1e-6
        );
      });
      expect(clamp, `variant ${String(v)}`).toHaveLength(1);
      const band = clamp[0];
      if (!band) throw new Error("no clamp");
      const outer = bounds(band).hi[v === 0 ? 2 : 1];
      const buttons = parts.filter((p) => {
        const c = centre(p);
        const { lo, hi } = bounds(p);
        const small = ([0, 1, 2] as const).every((j) => hi[j] - lo[j] < 0.012);
        return (
          small && c[k] > c0 && c[k] < c1 && hi[v === 0 ? 2 : 1] > outer + 1e-3
        );
      });
      expect(buttons, `variant ${String(v)}`).toHaveLength(1);
      const button = buttons[0];
      if (!button) throw new Error("no button");
      expect(meets(button, band)).toBe(true);
    }
  });

  it("lies in its cradle with nothing over the prongs, its eye the one light", () => {
    const parts = partsOf("light-sword", 0);
    const { top } = curioHalf("light-sword", 0);
    const hs = parts.flatMap(local).map((p) => p[2]);
    expect(Math.max(...hs)).toBeLessThanOrEqual(top + 1e-9);
    const lights = parts.filter(blinks);
    expect(lights).toHaveLength(1);
    expect(lights[0]?.flag).toBe(FLAG.blink);
  });

  it("stands the blade on the emitter, and the blue and green swords differ only in its tint", () => {
    for (const v of [1, 2]) {
      const parts = partsOf("light-sword", v);
      const lights = parts.filter(blinks);
      expect(lights, `variant ${String(v)}`).toHaveLength(1);
      const blade = lights[0];
      if (!blade) throw new Error("no blade");
      expect(blade.flag).toBe(FLAG.blink);
      const { lo, hi } = bounds(blade);
      expect(hi[2] - lo[2]).toBeGreaterThanOrEqual(0.85);
      const emitterTop = SWORD.standBase + HILT.length;
      expect(lo[2]).toBeCloseTo(emitterTop, 6);
      const hosts = parts.filter(
        (p) => p.flag === FLAG.lit && bounds(p).hi[2] >= emitterTop - 1e-6,
      );
      expect(hosts.some((p) => meets(blade, p))).toBe(true);
    }
    const strip = (ps: Part[]) =>
      ps.map((p) => ({ method: p.method, flag: p.flag, points: p.points }));
    expect(strip(partsOf("light-sword", 2))).toEqual(
      strip(partsOf("light-sword", 1)),
    );
    const blue = buildCurioMesh("light-sword", 1, LOOKS.aperture);
    const green = buildCurioMesh("light-sword", 2, LOOKS.aperture);
    expect(green.count).toBe(blue.count);
    expect(Array.from(green.vertices)).not.toEqual(Array.from(blue.vertices));
  });

  it("sets the pistol's glowing chamber on the body's top as its highest part", () => {
    const parts = partsOf("green-pistol", 0);
    const lights = parts.filter(blinks);
    expect(lights).toHaveLength(1);
    const bulb = lights[0];
    if (!bulb) throw new Error("no bulb");
    const { lo, hi } = bounds(bulb);
    expect(lo[2]).toBeCloseTo(PISTOL.body.h[1], 6);
    expect(lo[0]).toBeGreaterThan(PISTOL.body.a[0]);
    expect(hi[0]).toBeLessThan(PISTOL.body.a[1]);
    const others = parts.filter((p) => p !== bulb);
    for (const p of others) expect(bounds(p).hi[2]).toBeLessThan(hi[2] - 1e-3);
    expect(hi[2]).toBeCloseTo(curioHalf("green-pistol", 0).top, 6);
  });

  it("sets the gadget's top bulb off its base's axis", () => {
    const parts = partsOf("pink-gadget", 0);
    const bulbs = parts.filter(
      (p) =>
        p.method === "lathe" &&
        Math.abs(bounds(p).lo[2] - GADGET.bulb.h0) < 1e-6,
    );
    expect(bulbs).toHaveLength(1);
    const bulb = bulbs[0];
    if (!bulb) throw new Error("no bulb");
    const base = parts.filter(
      (p) => p.method === "box" && Math.abs(bounds(p).lo[2]) < 1e-9,
    );
    expect(base).toHaveLength(1);
    const [ba, bd] = centre(base[0] ?? bulb);
    const [ta, td] = centre(bulb);
    expect(Math.hypot(ta - ba, td - bd)).toBeGreaterThan(0.005);
  });

  it("stands the gadget cluster on three bases", () => {
    const parts = partsOf("pink-gadget", 1);
    const bases = parts.filter(
      (p) => p.method === "box" && Math.abs(bounds(p).lo[2]) < 1e-9,
    );
    expect(bases).toHaveLength(3);
    const as = bases.map((p) => centre(p)[0]).sort((x, y) => x - y);
    expect(as[0]).toBeCloseTo(-0.12, 6);
    expect(as[1]).toBeCloseTo(0, 6);
    expect(as[2]).toBeCloseTo(0.12, 6);
  });

  it("carries seven wing lights, each on its wing, chasing in the jumping order", () => {
    const parts = partsOf("wing-meter", 0);
    const lights = parts.filter(blinks);
    expect(lights).toHaveLength(7);
    const groups = lights.map((p) => p.flag - FLAG.blink).sort((x, y) => x - y);
    expect(groups).toEqual([0, 1, 2, 3, 4, 5, 6]);
    // A wing is a long lit extrusion on one side of the body.
    const wings = parts.filter((p) => {
      const { lo, hi } = bounds(p);
      return (
        p.method === "extrude" && p.flag === FLAG.lit && hi[0] - lo[0] > 0.08
      );
    });
    expect(wings).toHaveLength(2);
    const side = (p: Part) => Math.sign(centre(p)[0]);
    // Positions 1 to 4 run root to tip on the left wing (`-a`, the
    // viewer's left seen from the front), 5 to 7 on the right.
    const left = lights
      .filter((p) => side(p) < 0)
      .sort((p, q) => centre(q)[0] - centre(p)[0]);
    const right = lights
      .filter((p) => side(p) > 0)
      .sort((p, q) => centre(p)[0] - centre(q)[0]);
    expect(left).toHaveLength(4);
    expect(right).toHaveLength(3);
    // The jumping order itself, pinned as a literal so a change fails here.
    expect(METER.chase).toEqual([3, 5, 7, 4, 1, 6, 2]);
    [...left, ...right].forEach((p, i) => {
      expect(p.flag - FLAG.blink).toBe(METER.chase.indexOf(i + 1));
      const wing = wings.find((w) => side(w) === side(p));
      if (!wing) throw new Error("no wing");
      expect(meets(p, wing)).toBe(true);
      expect(centre(p)[1]).toBeGreaterThan(0);
    });
  });
});
