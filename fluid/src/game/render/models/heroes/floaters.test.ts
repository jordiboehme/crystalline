/**
 * The floaters' shape tests: what `heroModels.test.ts` does not check
 * for every kind. Their lift and the float check from it are the
 * general test's; here, the block's mark and where it is drawn, the
 * board's kicks and the cloud's tail.
 */

import { describe, expect, it } from "vitest";

import { FLAG, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { partsOf, toLocal, type Part } from "../../modelChecks";
import { QUESTION_MARK } from "./floaters";

const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

describe("floating hero models", () => {
  it("draws the mark as the question mark with its shadow one pixel down and right", () => {
    // Mutation caught: a shadow cell off by one, or a glyph cell lost.
    expect(QUESTION_MARK).toEqual([
      ".####..",
      "##ss##.",
      ".ss.##s",
      "...##ss",
      "..##ss.",
      "...ss..",
      "..##...",
      "...ss..",
    ]);
    QUESTION_MARK.forEach((row, y) =>
      [...row].forEach((ch, x) => {
        const up = QUESTION_MARK[y - 1]?.[x - 1];
        if (ch === "s") expect(up, `${x},${y}`).toBe("#");
        if (ch === "." && up === "#")
          throw new Error(`missing shadow at ${x},${y}`);
      }),
    );
  });

  it("shows the mark on the block's four sides and never on its top or bottom", () => {
    // Mutation caught: a side without its mark, or a mark on the top.
    const marks = partsOf("question-block").filter((p) => p.flag >= FLAG.blink);
    const sides = new Set<string>();
    for (const p of marks) {
      const pts = local(p);
      const hs = pts.map((q) => q[2]);
      expect(Math.max(...hs) - Math.min(...hs)).toBeGreaterThan(0);
      const as = pts.map((q) => q[0]);
      const ds = pts.map((q) => q[1]);
      if (Math.max(...as) - Math.min(...as) < 1e-6)
        sides.add(`a${Math.sign(as[0] ?? 0)}`);
      else if (Math.max(...ds) - Math.min(...ds) < 1e-6)
        sides.add(`d${Math.sign(ds[0] ?? 0)}`);
      else throw new Error("a mark quad that faces up or down");
    }
    expect([...sides].sort()).toEqual(["a-1", "a1", "d-1", "d1"]);
  });

  it("lifts both ends of the board by the same kick", () => {
    // Mutation caught: one kick missing or the deck tilted.
    const pts = partsOf("hoverboard").flatMap(local);
    const end = (sign: number) =>
      Math.max(...pts.filter((q) => sign * q[0] > 0.4).map((q) => q[2]));
    expect(end(1)).toBeCloseTo(end(-1), 4);
    const mid = Math.max(
      ...pts.filter((q) => Math.abs(q[0]) < 0.2).map((q) => q[2]),
    );
    expect(end(1)).toBeGreaterThan(mid + 0.03);
  });

  it("curls the cloud's tail up and to one side at its back", () => {
    // Mutation caught: the tail at the front, or flat.
    const pts = partsOf("flying-cloud").flatMap(local);
    const tail = pts.filter((q) => q[1] < -0.6);
    const body = pts.filter((q) => q[1] > -0.4);
    expect(tail.length).toBeGreaterThan(0);
    expect(Math.max(...tail.map((q) => q[2]))).toBeGreaterThan(
      Math.max(...body.map((q) => q[2])),
    );
    const mean = tail.reduce((s, q) => s + q[0], 0) / tail.length;
    expect(Math.abs(mean)).toBeGreaterThan(0.1);
  });
});
