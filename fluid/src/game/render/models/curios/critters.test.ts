/**
 * The critters' own shape tests: what `curioModels.test.ts`'s generic
 * checks do not pin for the hover drone and the soot puffs. Parts are
 * found by their lighting flag, their tint and their primitive, in the
 * recipe's own local `(a, d, h)` terms. The drone hovers wholly at its
 * lift, its shell split into plates round a round dark core, its eye
 * breathing on its front. The puffs huddle three or five fuzzy black
 * balls on the floor, each with two white eyes and black pupils facing
 * the front, a few crumbs round them.
 */

import { describe, expect, it } from "vitest";

import { curioLift } from "../../../world/curios";
import type { CurioKind } from "../../../world/types";
import { BLINK_GROUPS, bankSlot, createBlink } from "../../blink";
import { blinkFlag, createBuilder, FLAG, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOK } from "../../looks";
import { recordingKitAt, toLocal, type Part } from "../../modelChecks";
import { curioHalf } from "./common";
import {
  CORE_DARK,
  CRUMB,
  EYE_WHITE,
  PUPIL_BLACK,
  SHELL_GREY,
  SOOT_BLACK,
} from "./critters";
import { buildCurio } from "./index";

/** A curio kind's recorded parts, built once at the origin. */
function recorded(kind: CurioKind, variant = 0): Part[] {
  const builder = createBuilder();
  const parts: Part[] = [];
  buildCurio(recordingKitAt(builder, parts), kind, variant, LOOK);
  return parts;
}

/** A part's points in the recipe's local `[a, d, h]`. */
const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/** The low end, high end and middle of some points along axis `i`. */
const span = (pts: readonly V3[], i: 0 | 1 | 2) => {
  const v = pts.map((q) => q[i]);
  return {
    lo: Math.min(...v),
    hi: Math.max(...v),
    mid: (Math.min(...v) + Math.max(...v)) / 2,
  };
};

describe("the critters' models", () => {
  it("hovers the whole drone at its lift, its eye breathing on its front", () => {
    // Mutation caught: the eye on the back, or a part hanging below the lift.
    const parts = recorded("hover-drone");
    expect(span(parts.flatMap(local), 2).lo).toBeCloseTo(
      curioLift("hover-drone"),
      6,
    );
    const eye = parts.filter((p) => p.flag === blinkFlag(0));
    expect(eye.length).toBeGreaterThan(0);
    const { hd } = curioHalf("hover-drone", 0);
    expect(Math.max(...eye.flatMap(local).map((q) => q[1]))).toBeGreaterThan(
      hd - 0.02,
    );
  });

  it("sets the drone's breathing eye over a steady blue base it never shows darker than", () => {
    // Mutation caught: the eye's steady base removed, or a breathing part
    // whose low phase reads darker than the base behind it.
    const blink = createBlink();
    const channel = bankSlot("breathe") * BLINK_GROUPS;
    let low = 1;
    for (let t = 0; t < 2000; t++) {
      low = Math.min(low, blink.gains[channel] ?? 1);
      blink.tick();
    }
    const parts = recorded("hover-drone");
    const eye = parts.filter((p) => p.flag === blinkFlag(0));
    expect(eye.length).toBeGreaterThan(0);
    for (const part of eye) {
      const q = local(part);
      const [a, h, d] = [span(q, 0).mid, span(q, 2).mid, span(q, 1).lo];
      const bases = parts.filter((p) => {
        if (p.flag !== FLAG.signal || p.tint === null) return false;
        const b = local(p);
        return (
          span(b, 0).lo < a &&
          span(b, 0).hi > a &&
          span(b, 2).lo < h &&
          span(b, 2).hi > h &&
          span(b, 1).hi <= span(q, 1).hi &&
          span(b, 1).lo < d
        );
      });
      expect(bases, "a steady base behind the eye").toHaveLength(1);
      const base = bases[0]?.tint;
      const tint = part.tint;
      if (!base || !tint) throw new Error("tinted eye and base");
      expect(Math.max(...base), "the base is lit").toBeGreaterThan(0.3);
      for (const i of [0, 1, 2] as const)
        expect((tint[i] ?? 0) * low).toBeGreaterThanOrEqual(
          (base[i] ?? 0) * 0.9,
        );
    }
  });

  it("splits the drone's shell into at least four plates around a round core", () => {
    // Mutation caught: the shell drawn as one closed ball.
    const parts = recorded("hover-drone");
    expect(
      parts.filter((p) => p.tint?.join() === SHELL_GREY.join()).length,
    ).toBeGreaterThanOrEqual(4);
    expect(
      parts.some(
        (p) => p.method === "lathe" && p.tint?.join() === CORE_DARK.join(),
      ),
    ).toBe(true);
  });

  it("huddles three puffs in v0 and five in v1, each on the floor with two white eyes and black pupils, crumbs around", () => {
    // Mutation caught: a puff floating, an eye missing, or no crumbs.
    for (const [v, n] of [
      [0, 3],
      [1, 5],
    ] as const) {
      const parts = recorded("soot-puffs", v);
      const bodies = parts.filter(
        (p) => p.method === "lathe" && p.tint?.join() === SOOT_BLACK.join(),
      );
      expect(bodies, `bodies ${String(v)}`).toHaveLength(n);
      for (const b of bodies)
        expect(span(local(b), 2).lo, `floor ${String(v)}`).toBeCloseTo(0, 6);
      expect(
        parts.filter((p) => p.tint?.join() === EYE_WHITE.join()),
        `eyes ${String(v)}`,
      ).toHaveLength(2 * n);
      expect(
        parts.filter((p) => p.tint?.join() === PUPIL_BLACK.join()),
        `pupils ${String(v)}`,
      ).toHaveLength(2 * n);
      const crumbs = parts.filter(
        (p) => p.tint?.join() === CRUMB.join() && span(local(p), 2).hi <= 0.006,
      );
      expect(crumbs.length, `crumbs ${String(v)}`).toBeGreaterThanOrEqual(3);
      const whites = parts.filter((p) => p.tint?.join() === EYE_WHITE.join());
      for (const w of whites)
        expect(span(local(w), 1).mid, "eyes face the front").toBeGreaterThan(0);
    }
  });
});
