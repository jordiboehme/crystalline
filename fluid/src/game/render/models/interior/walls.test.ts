/**
 * The console room's wall pieces' shape tests (2.6e C5 to C7, C24a to
 * C24c): the roundel wall's grid of fifteen recessed roundels and the few
 * of them that glow, the inner doors' two tall leaves standing proud of
 * their roundel surround with a grid of roundels each, and the scanner's
 * lit screen with no roundel behind its housing. Every piece is built at
 * the origin with each kit call recorded and measured in the piece's own
 * `(a, d, h)` terms.
 */

import { describe, expect, it } from "vitest";

import type { InteriorKind } from "../../../world/types";
import { FLAG, createBuilder } from "../../geometry";
import { frameAt } from "../../kit";
import { LOOKS } from "../../looks";
import { recordingKitAt, toLocal, type Part } from "../../modelChecks";
import { CONSOLE_WALL, ROUNDEL_FACE } from "./common";
import { buildInterior } from ".";
import {
  GLOWING_ROUNDELS,
  INNER_DOORS,
  ROUNDEL,
  SCANNER,
  scannerHousing,
} from "./walls";

/** A piece built at the origin with every kit call recorded. */
function wallParts(
  kind: Exclude<InteriorKind, "console">,
  variant: number,
): Part[] {
  const parts: Part[] = [];
  buildInterior(
    recordingKitAt(createBuilder(), parts),
    kind,
    variant,
    LOOKS.aperture,
  );
  return parts;
}

/** A part's box in the piece's local terms: `a` across (x at turn 0, mirrored), `d` out from the wall, `h` up. */
function extent(p: Part): {
  a0: number;
  a1: number;
  d0: number;
  d1: number;
  h0: number;
  h1: number;
} {
  const local = p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));
  const pick = (i: 0 | 1 | 2) => local.map((l) => l[i]);
  return {
    a0: Math.min(...pick(0)),
    a1: Math.max(...pick(0)),
    d0: Math.min(...pick(1)),
    d1: Math.max(...pick(1)),
    h0: Math.min(...pick(2)),
    h1: Math.max(...pick(2)),
  };
}

const near = (x: number, y: number, eps = 0.01) => Math.abs(x - y) <= eps;

/** Whether a part is painted exactly `tint`. */
const tinted = (p: Part, tint: readonly number[]) =>
  p.tint !== null && p.tint.every((c, i) => near(c, tint[i] ?? -1, 1e-6));

/**
 * A roundel's inner face: a part of `ROUNDEL_FACE` tint about
 * `ROUNDEL.face` wide (the inner disc inside the rim, C24a; a door leaf's
 * slightly smaller disc passes too).
 */
const isRoundelFace = (p: Part) =>
  tinted(p, ROUNDEL_FACE) &&
  near(extent(p).a1 - extent(p).a0, ROUNDEL.face, 0.05);

/** A roundel's rim: an extruded part in the wall's white. */
const isRim = (p: Part) => p.method === "extrude" && tinted(p, CONSOLE_WALL);

/** A part's centre in `(a, h)`. */
const centreOf = (p: Part) => {
  const b = extent(p);
  return { a: (b.a0 + b.a1) / 2, h: (b.h0 + b.h1) / 2 };
};

const inside = (
  c: { a: number; h: number },
  b: { a0: number; a1: number; h0: number; h1: number },
) => c.a > b.a0 && c.a < b.a1 && c.h > b.h0 && c.h < b.h1;

const round3 = (x: number) => Math.round(x * 1000) / 1000;

describe("the console room's wall pieces (2.6e C5 to C7)", () => {
  it("sets fifteen roundels on a wall piece, three across and five up (2.6e C5)", () => {
    // Mutation caught: a row or a column missing, or the grid off centre.
    const discs = wallParts("roundel-wall", 0).filter((p) => isRoundelFace(p));
    expect(discs.length).toBe(ROUNDEL.cols * ROUNDEL.rows);
    const centres = discs.map(centreOf);
    const as = [...new Set(centres.map((c) => round3(c.a)))].sort(
      (x, y) => x - y,
    );
    expect(as).toEqual([-ROUNDEL.pitchA, 0, ROUNDEL.pitchA].map(round3));
    const hs = [...new Set(centres.map((c) => round3(c.h)))].sort(
      (x, y) => x - y,
    );
    expect(hs).toEqual(
      Array.from({ length: ROUNDEL.rows }, (_, r) =>
        round3(ROUNDEL.low + r * ROUNDEL.pitchH),
      ),
    );
  });

  it("builds a roundel 0.49 m across with a 0.08 m rim standing proud of its face (C24a)", () => {
    // Mutation caught: the rim at the plan's first 0.03 m or 0.46 m, or a
    // rim flush with (or behind) the face, which would not read recessed.
    const parts = wallParts("roundel-wall", 0);
    const face = parts.find(isRoundelFace);
    expect(face).toBeDefined();
    const c = centreOf(face!);
    const rims = parts.filter(
      (p) =>
        isRim(p) &&
        near(centreOf(p).a, c.a, 0.3) &&
        near(centreOf(p).h, c.h, 0.3),
    );
    // Two halves of one ring.
    expect(rims.length).toBe(2);
    const a0 = Math.min(...rims.map((p) => extent(p).a0));
    const a1 = Math.max(...rims.map((p) => extent(p).a1));
    expect(a1 - a0).toBeCloseTo(ROUNDEL.across, 3);
    expect(ROUNDEL.across).toBeCloseTo(0.49, 3);
    expect(ROUNDEL.rim).toBeCloseTo(0.08, 3);
    // The face shows through the ring's hole and hides under the ring.
    const f = extent(face!);
    expect(f.a1 - f.a0).toBeGreaterThan(ROUNDEL.across - 2 * ROUNDEL.rim);
    expect(f.a1 - f.a0).toBeLessThan(ROUNDEL.across);
    for (const rim of rims)
      expect(extent(rim).d1).toBeGreaterThan(f.d1 + 0.004);
  });

  it("lights exactly its variant's roundels, on the breathe bank's groups", () => {
    // Mutation caught: every roundel glowing, or none in variants 1 and 2.
    expect(GLOWING_ROUNDELS.length).toBe(3);
    GLOWING_ROUNDELS.forEach((cells, v) => {
      const lit = wallParts("roundel-wall", v).filter(
        (p) => p.flag >= FLAG.blink,
      );
      expect(lit.length).toBe(cells.length);
      // Each on a group of its own, so the two breathe out of step.
      expect(new Set(lit.map((p) => p.flag)).size).toBe(cells.length);
    });
    expect(GLOWING_ROUNDELS[0]).toEqual([]);
    expect(GLOWING_ROUNDELS[1]?.length).toBe(2);
    expect(GLOWING_ROUNDELS[2]?.length).toBe(2);
  });

  it("puts each lit roundel where its variant's cell says", () => {
    // Mutation caught: the column and the row swapped, or the glow drawn
    // on a roundel the variant does not name.
    for (const v of [1, 2]) {
      const cells = GLOWING_ROUNDELS[v] ?? [];
      expect(cells.length).toBeGreaterThan(0);
      const lit = wallParts("roundel-wall", v)
        .filter((p) => p.flag >= FLAG.blink)
        .map(centreOf)
        .map((c) => `${String(round3(c.a))},${String(round3(c.h))}`)
        .sort();
      const want = cells
        .map(
          ([col, row]) =>
            `${String(round3((col - 1) * ROUNDEL.pitchA))},${String(round3(ROUNDEL.low + row * ROUNDEL.pitchH))}`,
        )
        .sort();
      expect(lit).toEqual(want);
    }
  });

  it("builds the inner doors as two tall leaves with roundels, standing proud of the surround (2.6e C6)", () => {
    // Mutation caught: one leaf, leaves flush with the surround, or leaves
    // without roundels.
    const parts = wallParts("inner-doors", 0);
    const leaves = parts.filter((p) => {
      const b = extent(p);
      return (
        near(b.a1 - b.a0, INNER_DOORS.leafWidth) &&
        near(b.h1 - b.h0, INNER_DOORS.leafHeight)
      );
    });
    expect(leaves.length).toBe(2);
    // The surround: the one slab as wide as the piece.
    const surround = parts.filter((p) =>
      near(extent(p).a1 - extent(p).a0, 4.0, 0.02),
    );
    expect(surround.length).toBe(1);
    const surroundFront = extent(surround[0]!).d1;
    for (const leaf of leaves) {
      const b = extent(leaf);
      expect(b.d1 - surroundFront).toBeCloseTo(INNER_DOORS.proud, 2);
      const onLeaf = parts.filter(
        (p) => isRoundelFace(p) && inside(centreOf(p), b),
      );
      expect(onLeaf.length).toBe(INNER_DOORS.cols * INNER_DOORS.rows);
    }
  });

  it("keeps every leaf roundel 0.05 m clear of its leaf's edges and its neighbour (C24b)", () => {
    // Mutation caught: the leaf's roundels at the wall's full 0.49 m,
    // which leave only 0.04 m a side on a 1.1 m leaf, or a leaf 3 m tall.
    expect(INNER_DOORS.leafHeight).toBeCloseTo(2.7, 3);
    const parts = wallParts("inner-doors", 0);
    const leaves = parts
      .map(extent)
      .filter(
        (b) =>
          near(b.a1 - b.a0, INNER_DOORS.leafWidth) &&
          near(b.h1 - b.h0, INNER_DOORS.leafHeight),
      );
    expect(leaves.length).toBe(2);
    for (const leaf of leaves) {
      const rims = parts
        .filter((p) => isRim(p) && inside(centreOf(p), leaf))
        .map(extent);
      // Two halves of each of the leaf's roundels.
      expect(rims.length).toBe(2 * INNER_DOORS.cols * INNER_DOORS.rows);
      for (const r of rims) {
        expect(r.a0 - leaf.a0).toBeGreaterThanOrEqual(0.05 - 1e-6);
        expect(leaf.a1 - r.a1).toBeGreaterThanOrEqual(0.05 - 1e-6);
      }
      // Between the two columns: the gap from one ring's right edge to
      // the next ring's left edge.
      const lefts = [...new Set(rims.map((r) => round3(r.a0)))].sort(
        (x, y) => x - y,
      );
      const rights = [...new Set(rims.map((r) => round3(r.a1)))].sort(
        (x, y) => x - y,
      );
      expect(lefts.length).toBeGreaterThan(1);
      expect(lefts[lefts.length - 1]! - rights[0]!).toBeGreaterThanOrEqual(
        0.05 - 1e-3,
      );
    }
  });

  it("gives the scanner a lit screen with no roundel behind it (2.6e C7)", () => {
    // Mutation caught: the screen lit with a plain tint (it would read dark),
    // or a roundel left under the screen.
    const parts = wallParts("scanner", 0);
    const screens = parts.filter(
      (p) =>
        p.flag === FLAG.signal &&
        near(extent(p).a1 - extent(p).a0, SCANNER.width, 0.02),
    );
    expect(screens.length).toBe(1);
    const s = extent(screens[0]!);
    expect((s.h0 + s.h1) / 2).toBeCloseTo(SCANNER.centre, 2);
    expect(s.h1 - s.h0).toBeCloseTo(SCANNER.height, 2);
    const under = parts.filter(
      (p) => isRoundelFace(p) && inside(centreOf(p), s),
    );
    expect(under).toEqual([]);
  });

  it("leaves out every roundel the scanner's housing would cover, and only those (C24c)", () => {
    // Mutation caught: C7's middle column alone left out, so the side
    // columns' roundels run under the 1.32 m housing; or roundels dropped
    // that the housing does not reach.
    const parts = wallParts("scanner", 0);
    const box = scannerHousing();
    const housing = parts.filter((p) => {
      const b = extent(p);
      return (
        near(b.a0, box.a0) &&
        near(b.a1, box.a1) &&
        near(b.h0, box.h0) &&
        near(b.h1, box.h1)
      );
    });
    expect(housing.length).toBeGreaterThan(0);
    const rims = parts.filter(isRim).map(extent);
    expect(rims.length).toBeGreaterThan(0);
    for (const r of rims) {
      const overlaps =
        r.a0 < box.a1 && r.a1 > box.a0 && r.h0 < box.h1 && r.h1 > box.h0;
      expect(overlaps).toBe(false);
    }
    // Every cell the housing misses keeps its roundel.
    const faces = parts.filter(isRoundelFace).map(centreOf);
    let kept = 0;
    for (let col = 0; col < ROUNDEL.cols; col++)
      for (let row = 0; row < ROUNDEL.rows; row++) {
        const a = (col - 1) * ROUNDEL.pitchA;
        const h = ROUNDEL.low + row * ROUNDEL.pitchH;
        const half = ROUNDEL.across / 2;
        const covered =
          a - half < box.a1 &&
          a + half > box.a0 &&
          h - half < box.h1 &&
          h + half > box.h0;
        if (covered) continue;
        kept++;
        expect(
          faces.some((c) => near(c.a, a, 1e-3) && near(c.h, h, 1e-3)),
          `${String(col)},${String(row)}`,
        ).toBe(true);
      }
    expect(faces.length).toBe(kept);
  });

  it("dots the scanner's screen with small lit dots in front of it, and nothing else on it", () => {
    // Mutation caught: the dots drawn in a plain tint (dark under the
    // light), or lifted off the screen by less than `DECAL_LIFT`, so they
    // flicker through it at a distance.
    const parts = wallParts("scanner", 0);
    const screen = parts.find(
      (p) =>
        p.flag === FLAG.signal &&
        near(extent(p).a1 - extent(p).a0, SCANNER.width, 0.02),
    );
    expect(screen).toBeDefined();
    const s = extent(screen!);
    const dots = parts.filter((p) => {
      const b = extent(p);
      return (
        b.a1 - b.a0 < 0.05 && inside(centreOf(p), s) && b.d0 >= s.d1 - 1e-6
      );
    });
    expect(dots.length).toBe(SCANNER.dots);
    for (const d of dots) {
      expect(d.flag).toBe(FLAG.signal);
      expect(extent(d).d0 - s.d1).toBeGreaterThanOrEqual(0.01 - 1e-6);
    }
  });
});
