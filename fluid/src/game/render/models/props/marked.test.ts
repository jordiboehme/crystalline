/**
 * The marked props' own shape tests: what `propModels.test.ts`'s generic
 * checks do not pin for the marked crate and the gravity console. Parts
 * are found by their lighting flag and their tint, in the recipe's own
 * local `(a, d, h)` terms. The crate carries the maker's round C and its
 * word on all four of its sides, on the top crate when it stands on a
 * stack. The console blinks its readout's pixels in one group and two
 * buttons in two more, slants its deck up to the back over a waist-high
 * pillar, and carries the dial's one needle above the pillar and the mark
 * on the pillar below the deck.
 */

import { describe, expect, it } from "vitest";

import type { PropKind } from "../../../world/types";
import { blinkFlag, createBuilder, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOKS } from "../../looks";
import { recordingKitAt, toLocal, type Part } from "../../modelChecks";
import { pixelRuns, textRows } from "../heroes/pixels";
import { MARK_BLUE, MARKS } from "../marks";
import { buildProp } from "./index";
import { DECK_GREY, NEEDLE, READOUT } from "./marked";

/** A prop kind's recorded parts, built once at the origin. */
function recorded(kind: PropKind, variant: number): Part[] {
  const builder = createBuilder();
  const parts: Part[] = [];
  buildProp(recordingKitAt(builder, parts), kind, variant, {
    look: LOOKS.aperture,
  });
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

describe("the marked props' models", () => {
  it("stencils the round C and the word on all four sides of the crate, on the top crate of the stack", () => {
    // Mutation caught: a side left blank, or the mark on the stack's base.
    const runs = pixelRuns(textRows(MARKS.capsuleWord)).length;
    const flat = (q: readonly V3[], i: 0 | 1) =>
      span(q, i).hi - span(q, i).lo < 1e-6;
    for (const v of [0, 1]) {
      const marks = recorded("marked-crate", v).filter(
        (p) => p.tint?.join() === MARK_BLUE.join(),
      );
      const faces = new Map<string, number>();
      for (const p of marks) {
        const q = local(p);
        const face =
          flat(q, 1) && span(q, 1).lo > 0
            ? "front"
            : flat(q, 1) && span(q, 1).hi < 0
              ? "back"
              : flat(q, 0) && span(q, 0).lo > 0
                ? "right"
                : flat(q, 0) && span(q, 0).hi < 0
                  ? "left"
                  : null;
        if (face === null)
          throw new Error(`a mark part off every face (variant ${String(v)})`);
        faces.set(face, (faces.get(face) ?? 0) + 1);
        if (v === 1)
          expect(span(q, 2).lo, "on the top crate").toBeGreaterThan(0.85);
      }
      expect([...faces.keys()].sort(), String(v)).toEqual([
        "back",
        "front",
        "left",
        "right",
      ]);
      for (const [face, n] of faces)
        expect(n, `${face} ${String(v)}`).toBeGreaterThanOrEqual(runs);
    }
  });

  it("blinks the readout's pixels in group 0 and two buttons in groups 1 and 2", () => {
    // Mutation caught: the readout drawn steady, or a button in the
    // readout's group.
    const parts = recorded("gravity-console", 0);
    const g = (n: number) => parts.filter((p) => p.flag === blinkFlag(n));
    expect(g(0)).toHaveLength(pixelRuns(textRows(MARKS.gravity)).length);
    expect(g(1)).toHaveLength(1);
    expect(g(2)).toHaveLength(1);
    expect(parts.filter((p) => p.flag > blinkFlag(2))).toEqual([]);
    for (const p of g(0)) expect(p.tint).toEqual(READOUT);
  });

  it("slants the deck up to the back over a waist-high pillar, with the dial's needle above and the mark below", () => {
    // Mutation caught: a flat deck, the needle missing, or the mark on the
    // deck instead of the pillar.
    const parts = recorded("gravity-console", 0);
    const pts = parts.flatMap(local);
    const topAt = (lo: number, hi: number) =>
      Math.max(...pts.filter((q) => q[1] >= lo && q[1] <= hi).map((q) => q[2]));
    expect(topAt(-0.2, -0.1)).toBeGreaterThan(topAt(0.12, 0.22) + 0.08);
    // The facia behind the deck reaches the back band on its own, so the
    // deck's own points are held to the same rule.
    const deck = parts
      .filter((p) => p.tint?.join() === DECK_GREY.join())
      .flatMap(local);
    const deckTop = (lo: number, hi: number) =>
      Math.max(
        ...deck.filter((q) => q[1] >= lo && q[1] <= hi).map((q) => q[2]),
      );
    expect(deckTop(-0.2, -0.1)).toBeGreaterThan(deckTop(0.12, 0.22) + 0.08);
    expect(deckTop(0.12, 0.22)).toBeGreaterThanOrEqual(0.85);
    expect(parts.filter((p) => p.tint?.join() === NEEDLE.join())).toHaveLength(
      1,
    );
    const mark = parts.filter((p) => p.tint?.join() === MARK_BLUE.join());
    expect(mark.length).toBeGreaterThan(0);
    for (const p of mark) expect(span(local(p), 2).hi).toBeLessThan(0.85);
  });
});
