/**
 * The street heroes' shape tests: the bike's sourced size and its two
 * lights; the police box's sign layout, its sign on all four sides, and
 * its doors built by the helper 2.6e will animate.
 */

import { describe, expect, it } from "vitest";

import { FLAG, blinkFlag, type V3 } from "../../geometry";
import { frameAt } from "../../kit";
import { partsOf, toLocal, type Part } from "../../modelChecks";
import { BIKE, BOX_BACK, BOX_SIGN, BRAKE_STEEL, boxSignLayout } from "./street";
import { textRows } from "./pixels";

const local = (p: Part): V3[] =>
  p.points.map((q) => toLocal(frameAt([0, 0, 0], 0), q));

describe("street hero models", () => {
  it("builds the bike at the original's size: 2.95 long, 0.83 wide, the windscreen at 1.17", () => {
    // Mutation caught: a bike shorter than `BIKE.length`.
    const pts = partsOf("red-bike").flatMap(local);
    const as = pts.map((q) => q[0]);
    expect(Math.max(...as) - Math.min(...as)).toBeCloseTo(BIKE.length, 2);
    const body = partsOf("red-bike")
      .filter((p) => p.method === "extrude")
      .flatMap(local);
    const ds = body.map((q) => q[1]);
    expect(Math.max(...ds) - Math.min(...ds)).toBeLessThanOrEqual(
      BIKE.width + 1e-6,
    );
    expect(Math.max(...pts.map((q) => q[2]))).toBeGreaterThanOrEqual(
      BIKE.screenTop - 1e-6,
    );
  });

  it("lights the bike with a steady headlight at the nose and a breathing tail light at the back", () => {
    // Mutation caught: the tail light made steady (two steady lights),
    // or the two lights swapped end for end.
    const parts = partsOf("red-bike");
    const head = parts.filter((p) => p.flag === FLAG.signal);
    const tail = parts.filter((p) => p.flag === blinkFlag(0));
    expect(head).toHaveLength(1);
    expect(tail).toHaveLength(1);
    expect(
      Math.min(...local(head[0] as Part).map((q) => q[0])),
    ).toBeGreaterThan(1.3);
    expect(Math.max(...local(tail[0] as Part).map((q) => q[0]))).toBeLessThan(
      -1.3,
    );
  });

  it("fits one brake disc, on the front wheel, outboard of its hub", () => {
    // Mutation caught: the disc on the rear wheel, on both wheels, or
    // sunk inside the hub where it cannot be seen. The guard: exactly one
    // part in the disc's steel, so the test cannot pass on none.
    const discs = partsOf("red-bike").filter(
      (p) => p.tint !== null && p.tint.every((c, i) => c === BRAKE_STEEL[i]),
    );
    expect(discs).toHaveLength(1);
    const pts = local(discs[0] as Part);
    const as = pts.map((q) => q[0]);
    expect((Math.max(...as) + Math.min(...as)) / 2).toBeCloseTo(1.0, 2);
    expect(Math.max(...as) - Math.min(...as)).toBeLessThan(0.4);
    // Outboard of the hub's face at 0.17 on the `+d` side.
    expect(Math.min(...pts.map((q) => q[1]))).toBeGreaterThan(0.17);
  });

  it("lays the sign out as on the original: POLICE, PUBLIC over CALL, BOX, inside the band", () => {
    // Mutation caught: the words reordered, the stack side by side, or
    // the sign wider than its band.
    const words = boxSignLayout(1.1, 2.17);
    expect(words.map((w) => w.rows)).toEqual(
      [BOX_SIGN.left, BOX_SIGN.upper, BOX_SIGN.lower, BOX_SIGN.right].map(
        textRows,
      ),
    );
    const [left, upper, lower, right] = words;
    if (!left || !upper || !lower || !right) throw new Error("four words");
    expect(upper.a0).toBeCloseTo(lower.a0, 9);
    expect(upper.h1).toBeGreaterThan(lower.h1);
    expect(upper.px).toBeLessThan(left.px);
    const span = (w: (typeof words)[number]) => [
      w.a0,
      w.a0 + (w.rows[0]?.length ?? 0) * w.px,
    ];
    expect(span(left)[1]).toBeLessThan(upper.a0);
    expect(span(upper)[1]).toBeLessThan(right.a0);
    expect(left.a0).toBeGreaterThanOrEqual(-0.55 - 1e-9);
    expect(span(right)[1]).toBeLessThanOrEqual(0.55 + 1e-9);
  });

  it("puts the sign on all four sides of the box, the back one inside the envelope", () => {
    // Mutation caught: the back sign dropped, or drawn behind the wall plane.
    const letters = partsOf("police-box").filter(
      (p) => p.flag === FLAG.signal && p.method === "panel",
    );
    const faces = new Set<string>();
    for (const p of letters) {
      const pts = local(p);
      const hs = pts.map((q) => q[2]);
      if (Math.min(...hs) < 2.0) continue; // the windows glow too, lower down
      const as = pts.map((q) => q[0]);
      const ds = pts.map((q) => q[1]);
      if (Math.max(...ds) - Math.min(...ds) < 1e-6)
        faces.add((ds[0] ?? 0) > 0.65 ? "front" : "back");
      else if (Math.max(...as) - Math.min(...as) < 1e-6)
        faces.add((as[0] ?? 0) > 0 ? "a+" : "a-");
      for (const q of pts) expect(q[1]).toBeGreaterThanOrEqual(0);
    }
    expect([...faces].sort()).toEqual(["a+", "a-", "back", "front"]);
    expect(BOX_BACK).toBeGreaterThan(0.03);
  });

  it("keeps the box's use point clear: nothing reaches past its front", () => {
    // Mutation caught: a handle, a panel or the sign standing out past the
    // corner posts' front into the use point. The guard: the posts do
    // reach the front, so the test cannot pass on a box built short.
    const ds = partsOf("police-box")
      .flatMap(local)
      .map((q) => q[1]);
    expect(Math.max(...ds)).toBeGreaterThanOrEqual(1.3 - 1e-6);
    for (const d of ds) expect(d).toBeLessThanOrEqual(1.3 + 1e-6);
  });
});
