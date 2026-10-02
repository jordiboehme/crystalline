/**
 * The living heroes' shape tests: what `heroModels.test.ts` does not check
 * for every kind. Whether the mess table's catalogue surface lies on a
 * real, clear upward face is `heroModels.test.ts`'s check now, for every
 * kind; here, the drinking bird stands on the table, the robot's two
 * faces never share a quad, the sleep ring carries six pods and six
 * lights with each lit lid over its own pod, and the dome planters hold
 * two or three domes with a grow lamp under each.
 */

import { describe, expect, it } from "vitest";

import { FLAG, blinkFlag, createBuilder, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOK } from "../../looks";
import { partsOf, recordingKitAt, toLocal, type Part } from "../../modelChecks";
import { surfaces } from "../common";
import { BIRD_A, sleepPod } from "./living";

/** A part's points in the recipe's local `[a, d, h]`. */
const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/** The `(a, h)` plan box of a part's points, ignoring depth. */
function planBox(p: Part): { a0: number; a1: number; h0: number; h1: number } {
  const pts = local(p);
  const as = pts.map((q) => q[0]);
  const hs = pts.map((q) => q[2]);
  return {
    a0: Math.min(...as),
    a1: Math.max(...as),
    h0: Math.min(...hs),
    h1: Math.max(...hs),
  };
}

/** Whether two `(a, h)` plan boxes overlap. */
function overlaps(
  x: { a0: number; a1: number; h0: number; h1: number },
  y: { a0: number; a1: number; h0: number; h1: number },
): boolean {
  return x.a0 < y.a1 && y.a0 < x.a1 && x.h0 < y.h1 && y.h0 < x.h1;
}

describe("living hero models", () => {
  it("the bird stands on the table", () => {
    // The bird sits at a BIRD_A (1.15), d 0; a window of 0.1 either side
    // catches only its own parts and its water glass: the table's rounded
    // ends and skirt start at a 1.4, the spine ends at 1.5, the tray and
    // cups sit at a 1.45 to 1.75 and the catalogue surface ends at 0.9.
    const bird = partsOf("mess-table")
      .flatMap(local)
      .filter((p) => p[0] > BIRD_A - 0.1 && p[0] < BIRD_A + 0.1);
    expect(bird.length).toBeGreaterThan(0);
    const lowest = Math.min(...bird.map((p) => p[2]));
    const highest = Math.max(...bird.map((p) => p[2]));
    expect(lowest).toBeCloseTo(0.76, 3);
    expect(highest).toBeLessThanOrEqual(1.1 + 1e-6);
  });

  it("the robot's two faces never share a quad", () => {
    const parts = partsOf("helper-robot");
    const faceA = parts
      .filter((p) => p.flag >= blinkFlag(0) && p.flag <= blinkFlag(3))
      .map(planBox);
    const faceB = parts
      .filter((p) => p.flag >= blinkFlag(4) && p.flag <= blinkFlag(7))
      .map(planBox);
    expect(faceA.length).toBeGreaterThan(0);
    expect(faceB.length).toBeGreaterThan(0);
    for (const a of faceA)
      for (const b of faceB) expect(overlaps(a, b)).toBe(false);
  });

  it("the ring has six pods and six lights, one per group", () => {
    const parts = partsOf("sleep-ring");
    for (let group = 0; group < 6; group++) {
      const lights = parts.filter((p) => p.flag === blinkFlag(group));
      expect(lights, `group ${String(group)}`).toHaveLength(1);
    }
    for (let group = 6; group < 8; group++) {
      expect(parts.filter((p) => p.flag === blinkFlag(group))).toHaveLength(0);
    }
  });

  it("each pod's lit lid lies over its own shell, on the pod's outward heading", () => {
    // Build one pod at a time, so a lid built over the opposite pod cannot
    // be matched to a neighbour: all six pods look alike, but each is
    // checked on its own parts only.
    const look = surfaces(LOOK);
    const f = frameAt([0, 0, 0], 0);
    const centre = (p: Part) => {
      const pts = local(p);
      return [
        pts.reduce((t, q) => t + q[0], 0) / pts.length,
        pts.reduce((t, q) => t + q[1], 0) / pts.length,
      ] as const;
    };
    for (let pod = 0; pod < 6; pod++) {
      const parts: Part[] = [];
      sleepPod(recordingKitAt(createBuilder(), parts), f, look, pod);
      // The yawed frame's `inward` (the pod's outward `d`) in `(a, d)`.
      const out = [
        -Math.sin((pod * Math.PI) / 3),
        Math.cos((pod * Math.PI) / 3),
      ] as const;
      const along = (p: Part) => {
        const [a, d] = centre(p);
        return {
          reach: a * out[0] + d * out[1],
          off: -a * out[1] + d * out[0],
        };
      };
      const shells = parts.filter((p) => p.method === "bevelBox");
      const lids = parts.filter((p) => p.flag === FLAG.signal);
      const lights = parts.filter((p) => p.flag === blinkFlag(pod));
      expect(shells, `pod ${String(pod)}`).toHaveLength(1);
      expect(lids, `pod ${String(pod)}`).toHaveLength(2);
      expect(lights, `pod ${String(pod)}`).toHaveLength(1);
      for (const p of [...shells, ...lids, ...lights]) {
        const { reach, off } = along(p);
        expect(reach, `pod ${String(pod)} ${p.method}`).toBeGreaterThan(0.5);
        expect(Math.abs(off), `pod ${String(pod)} ${p.method}`).toBeLessThan(
          1e-6,
        );
      }
    }
  });

  it("dome planters have two or three domes, a grow lamp under each", () => {
    // Each dome is an open lattice of twenty struts (extruded bars) and
    // hangs one grow lamp, its own blink group, under its apex hub.
    for (const [variant, count] of [
      [0, 2],
      [1, 3],
    ] as const) {
      const parts = partsOf("dome-planters", variant);
      expect(
        parts.filter((p) => p.method === "extrude"),
        `variant ${String(variant)}`,
      ).toHaveLength(20 * count);
      for (let group = 0; group < 8; group++) {
        const lamps = parts.filter((p) => p.flag === blinkFlag(group));
        expect(
          lamps,
          `variant ${String(variant)} group ${String(group)}`,
        ).toHaveLength(group < count ? 1 : 0);
        for (const lamp of lamps) {
          const top = Math.max(...local(lamp).map((q) => q[2]));
          expect(top).toBeLessThan(1.5 - 0.2);
        }
      }
    }
  });
});
