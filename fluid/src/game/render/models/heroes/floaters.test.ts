/**
 * The floaters' shape tests: what `heroModels.test.ts` does not check
 * for every kind. Their lift and the float check from it are the
 * general test's; here, the block's mark and where it is drawn, the
 * board's kicks and the wordmark on its deck, and the cloud's tail.
 */

import { describe, expect, it } from "vitest";

import { heroLift } from "../../../world/footprints";
import { FLAG, type V3 } from "../../geometry";
import { DECAL_LIFT, frameAt } from "../../kit";
import { partsOf, toLocal, type Part } from "../../modelChecks";
import { MARKS } from "../marks";
import { DECK_LOGO_INK, QUESTION_MARK } from "./floaters";
import { pixelRuns, textBlock, textRows } from "./pixels";

const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

/** How many lit runs `lines` make in the font. */
const runsOfLines = (lines: string | readonly string[]): number =>
  (typeof lines === "string" ? [lines] : lines).reduce(
    (n, l) => n + pixelRuns(textRows(l)).length,
    0,
  );

/** The parts of `parts` painted exactly `ink`, whatever primitive drew them. */
const inked = (parts: readonly Part[], ink: readonly number[]): Part[] =>
  parts.filter((p) => p.tint?.join() === ink.join());

/**
 * A flat mark's pixel size: the smaller of its run's two extents across
 * the deck, `a` and `d` (a run is one pixel deep and one or more long).
 */
const pixelOf = (p: Part): number => {
  const ps = local(p);
  const span = (k: 0 | 1) =>
    Math.max(...ps.map((q) => q[k])) - Math.min(...ps.map((q) => q[k]));
  return Math.min(span(0), span(1));
};

/**
 * The board's flat middle, as `BOARD` builds it on the 0.9 by 0.25
 * footprint: `a` within the flat's 0.12 of the centre, `d` within the
 * deck's half width (0.125 less the 0.015 inset), its top at the lift
 * plus the deck's 0.035.
 */
const FLAT = { a: 0.12, d: 0.11, top: 0.25 + 0.035 };

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
    // Mutation caught: one kick missing or the deck tilted, and a middle
    // with no deck (the empty set's -Infinity would pass the kick check).
    // The wordmark's layer on the flat is left out: it is no deck.
    const pts = partsOf("hoverboard")
      .filter(
        (p) =>
          !local(p).every(
            (q) => Math.abs(q[0]) <= FLAT.a + 1e-9 && q[2] >= FLAT.top - 1e-9,
          ),
      )
      .flatMap(local);
    const end = (sign: number) =>
      Math.max(...pts.filter((q) => sign * q[0] > 0.4).map((q) => q[2]));
    expect(end(1)).toBeCloseTo(end(-1), 4);
    const middle = pts.filter((q) => Math.abs(q[0]) < 0.2).map((q) => q[2]);
    expect(middle.length).toBeGreaterThan(0);
    // The deck's middle lies on the lift, and it is a thin flat deck.
    const lift = heroLift("hoverboard");
    expect(Math.min(...middle)).toBeCloseTo(lift, 6);
    expect(Math.max(...middle)).toBeLessThan(lift + 0.05);
    const mid = Math.max(...middle);
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

  it("prints the wordmark on the board's deck between the pads, over the floor (2.6f C13)", () => {
    // Mutation caught: the wordmark missing, set from the wrong lines, on
    // a kick instead of the flat, off its patch (sunk into it or floating
    // over it), or below the floor.
    expect(runsOfLines(MARKS.boardLogo)).toBeGreaterThan(3);
    const ink = inked(partsOf("hoverboard"), DECK_LOGO_INK);
    expect(ink).toHaveLength(runsOfLines(MARKS.boardLogo));
    for (const p of ink) {
      expect(pixelOf(p)).toBeGreaterThanOrEqual(0.003 - 1e-9);
      for (const q of local(p)) {
        expect(Math.abs(q[0])).toBeLessThanOrEqual(FLAT.a + 1e-9);
        expect(Math.abs(q[1])).toBeLessThanOrEqual(FLAT.d + 1e-9);
        expect(q[2]).toBeGreaterThanOrEqual(FLAT.top + DECAL_LIFT - 1e-9);
        expect(q[2]).toBeLessThanOrEqual(FLAT.top + 2 * DECAL_LIFT + 1e-9);
      }
    }
  });

  it("reads the wordmark from the board's front: the first line far, left to right (2.6f C17)", () => {
    // Mutation caught: the lines swapped, or the block turned about or
    // mirrored, so it reads upside down or backwards from the front. The
    // lit cells are rebuilt from the boxes, the far edge as row 0 and the
    // low-`a` edge as column 0, and must be the block itself.
    const block = textBlock(MARKS.boardLogo);
    const ink = inked(partsOf("hoverboard"), DECK_LOGO_INK).map(local);
    const px = Math.min(
      ...ink.map(
        (ps) =>
          Math.max(...ps.map((q) => q[1])) - Math.min(...ps.map((q) => q[1])),
      ),
    );
    const a0 = Math.min(...ink.flatMap((ps) => ps.map((q) => q[0])));
    const d0 = Math.min(...ink.flatMap((ps) => ps.map((q) => q[1])));
    const grid = block.map((r) => [...r].map(() => "."));
    for (const ps of ink) {
      const row = Math.round((Math.min(...ps.map((q) => q[1])) - d0) / px);
      const c0 = Math.round((Math.min(...ps.map((q) => q[0])) - a0) / px);
      const c1 = Math.round((Math.max(...ps.map((q) => q[0])) - a0) / px);
      for (let c = c0; c < c1; c++) {
        const line = grid[row];
        if (line) line[c] = "#";
      }
    }
    expect(grid.map((r) => r.join(""))).toEqual(block);
  });
});
