/**
 * The keepsake batch's own shape tests: what `curioModels.test.ts`'s
 * generic checks do not pin for the star ball, the catch ball, the trap
 * box and the fuel case. Parts are found by method, layer, shape and,
 * for the fuel case's marks, by colour (`recordingKitAt` keeps each kit
 * call's surface tint), in the recipe's own local `(a, d, h)` terms
 * (`toLocal`). The fuel case's three labels (2.6f C13), each rebuilt
 * from its parts cell by cell, must be the font's rows of its line, on
 * the face the original carries it, one `MARK_PROUD` off that face and
 * at least 1 mm a pixel.
 */

import { describe, expect, it } from "vitest";

import type { CurioKind } from "../../../world/types";
import { createBuilder, type V3 } from "../../geometry";
import { frameAt, type Frame } from "../../kit";
import { LAYER } from "../../layers";
import { LOOKS } from "../../looks";
import {
  cellsOf,
  inked,
  recordingKitAt,
  runsOfLines,
  shape,
  toLocal,
  trimmed,
  type Part,
  type Shape,
} from "../../modelChecks";
import { MARK_PROUD, textRows } from "../heroes/pixels";
import { MARKS } from "../marks";
import { curioHalf } from "./common";
import { buildCurio } from "./index";
import {
  CASE_LABEL_INK,
  LABEL_YELLOW,
  STICKER_CREAM,
  STRIPE_RED,
  TREFOIL_BLACK,
} from "./keepsakes";

/** Every curio in this batch is built in this frame, at the origin. */
const ORIGIN: Frame = frameAt([0, 0, 0], 0);

/** A keepsake kind's recorded parts, built once at the origin. */
function partsOf(kind: CurioKind): Part[] {
  const parts: Part[] = [];
  buildCurio(recordingKitAt(createBuilder(), parts), kind, 0, LOOKS.aperture);
  return parts;
}

/** A part's shape in the recipe's own local `(a, d, h)` terms, not world metres. */
function localShape(p: Part): Shape {
  return shape(p.points.map((q) => toLocal(ORIGIN, q)));
}

/** A part's points in the recipe's own local `(a, d, h)` terms. */
const local = (p: Part): V3[] => p.points.map((q) => toLocal(ORIGIN, q));

/** A reader's `at` over local points, taking the recorded ones. */
const inLocal =
  (at: (q: V3) => readonly [x: number, y: number]) =>
  (q: V3): readonly [x: number, y: number] =>
    at(toLocal(ORIGIN, q));

describe("star ball", () => {
  it("sets four stars into the surface, its lowest point at 0", () => {
    const parts = partsOf("star-ball");
    const stars = parts.filter((p) => p.method === "extrude");
    expect(stars).toHaveLength(4);
    const lowest = Math.min(
      ...parts.flatMap((p) => p.points.map((q) => toLocal(ORIGIN, q)[2] ?? 0)),
    );
    expect(lowest).toBeCloseTo(0, 6);
  });
});

describe("catch ball", () => {
  it("is red above the band and white below", () => {
    const { top } = curioHalf("catch-ball", 0);
    const r = top / 2;
    const halves = partsOf("catch-ball")
      .filter((p) => p.method === "lathe")
      .map(localShape);
    expect(halves).toHaveLength(2);
    expect(halves.some((s) => s.lo[2] >= r - 1e-6)).toBe(true);
    expect(halves.some((s) => s.hi[2] <= r + 1e-6)).toBe(true);
  });
});

describe("trap box", () => {
  it("stripes both long sides with hazard panels", () => {
    const hazard = partsOf("trap-box")
      .filter((p) => p.layer === LAYER.hazard)
      .map(localShape);
    expect(hazard).toHaveLength(2);
    expect(hazard.some((s) => s.hi[1] <= 0)).toBe(true);
    expect(hazard.some((s) => s.lo[1] >= 0)).toBe(true);
  });

  it("touches the hazard panel with the two side tubes, no more than a millimetre apart", () => {
    const parts = partsOf("trap-box");
    const panel = parts
      .filter((p) => p.layer === LAYER.hazard)
      .map(localShape)
      .find((s) => s.hi[1] <= 0);
    if (!panel) throw new Error("back hazard panel missing");
    const tubes = parts
      .filter((p) => p.method === "cylinderAlong")
      .map(localShape)
      .filter((s) => s.hi[2] < 0.1);
    expect(tubes).toHaveLength(2);
    for (const tube of tubes)
      expect(Math.abs(tube.hi[1] - panel.lo[1])).toBeLessThan(0.001);
  });

  it("keeps the pedal apart from the trap, its cable touching both", () => {
    const parts = partsOf("trap-box");
    const trapBody = parts.find((p) => p.method === "bevelBox");
    const pedal = parts.find((p) => p.method === "extrude");
    if (!trapBody || !pedal) throw new Error("trap or pedal missing");
    const trapShape = localShape(trapBody);
    const pedalShape = localShape(pedal);
    expect(pedalShape.lo[0]).toBeGreaterThan(trapShape.hi[0]);

    const cable = parts
      .filter((p) => p.method === "box")
      .map(localShape)
      .filter((s) => s.hi[2] <= 0.01);
    expect(cable.length).toBeGreaterThanOrEqual(3);
    expect(Math.min(...cable.map((s) => s.lo[0]))).toBeCloseTo(
      trapShape.hi[0],
      5,
    );
    expect(Math.max(...cable.map((s) => s.hi[0]))).toBeCloseTo(
      pedalShape.lo[0],
      5,
    );
  });
});

describe("fuel case", () => {
  const parts = partsOf("fuel-case");
  const label = localShape(
    inked(parts, LABEL_YELLOW)[0] ??
      (() => {
        throw new Error("the label");
      })(),
  );
  /** The ink parts lying `MARK_PROUD` off the plane `axis = at`, on the side `out` (1 or -1) points to. */
  const onPlane = (ink: readonly Part[], axis: 0 | 1, at: number, out = 1) =>
    ink.filter((p) =>
      local(p).every((q) => Math.abs(q[axis] - (at + out * MARK_PROUD)) < 1e-9),
    );

  it("prints all three labels as the font's runs, no more (2.6f C13)", () => {
    // Mutation caught: a label left off, or drawn twice.
    expect(inked(parts, CASE_LABEL_INK)).toHaveLength(
      runsOfLines(MARKS.caseLabels),
    );
  });

  it("carries three trefoil sectors and a centre disc on the label", () => {
    const shapes = parts.filter((p) => p.method === "extrude").map(localShape);
    expect(shapes).toHaveLength(4);
    const diagonal = (s: Shape) =>
      Math.hypot(s.hi[0] - s.lo[0], s.hi[2] - s.lo[2]);
    const sectors = shapes.filter((s) => diagonal(s) > 0.025);
    const disc = shapes.filter((s) => diagonal(s) <= 0.025);
    expect(sectors).toHaveLength(3);
    expect(disc).toHaveLength(1);
  });

  it("prints the hazard class line on the yellow label under the trefoil, one mark proud (2.6f C13)", () => {
    // Mutation caught: the line missing, over the trefoil, off the label,
    // turned about or mirrored, floating or sunk, or under the 1 mm floor.
    const trefoil = parts.filter(
      (p) => p.method === "extrude" && p.tint?.join() === TREFOIL_BLACK.join(),
    );
    expect(trefoil).toHaveLength(4);
    const trefoilLow = Math.min(...trefoil.flatMap(local).map((q) => q[2]));
    const line = onPlane(inked(parts, CASE_LABEL_INK), 1, label.hi[1]);
    const text = cellsOf(
      line,
      inLocal((q) => [q[0], -q[2]]),
    );
    expect(text.rows).toEqual(trimmed(textRows(MARKS.caseLabels[0])));
    expect(text.px).toBeGreaterThanOrEqual(0.001 - 1e-9);
    for (const q of line.flatMap(local)) {
      expect(q[0]).toBeGreaterThanOrEqual(label.lo[0]);
      expect(q[0]).toBeLessThanOrEqual(label.hi[0]);
      expect(q[2]).toBeGreaterThanOrEqual(label.lo[2]);
      expect(q[2]).toBeLessThan(trefoilLow);
    }
  });

  it("prints the handling line on the case's front under the red stripe, one mark proud (2.6f C13)", () => {
    // Mutation caught: the line left off, on the stripe or over it, turned
    // about or mirrored, floating or sunk, or under the 1 mm floor.
    const body = parts.find((p) => p.method === "bevelBox");
    if (!body) throw new Error("the case body");
    const front = localShape(body).hi[1];
    const stripeLow = Math.min(
      ...inked(parts, STRIPE_RED)
        .flatMap(local)
        .map((q) => q[2]),
    );
    const line = onPlane(inked(parts, CASE_LABEL_INK), 1, front);
    const text = cellsOf(
      line,
      inLocal((q) => [q[0], -q[2]]),
    );
    expect(text.rows).toEqual(trimmed(textRows(MARKS.caseLabels[2])));
    expect(text.px).toBeGreaterThanOrEqual(0.001 - 1e-9);
    for (const q of line.flatMap(local)) expect(q[2]).toBeLessThan(stripeLow);
  });

  it("prints the caution line on a cream sticker on the case's own right side, reading from that side (2.6f C13)", () => {
    // Mutation caught: the sticker or its line left off, the line off the
    // sticker, turned about or mirrored, floating or sunk, or under the
    // 1 mm floor.
    const stickers = inked(parts, STICKER_CREAM).map(localShape);
    expect(stickers).toHaveLength(1);
    const sticker = stickers[0] as Shape;
    // The case faces `+d`, so its own right is `-a`.
    expect(sticker.hi[0]).toBeLessThan(0);
    const line = onPlane(inked(parts, CASE_LABEL_INK), 0, sticker.lo[0], -1);
    const text = cellsOf(
      line,
      inLocal((q) => [q[1], -q[2]]),
    );
    expect(text.rows).toEqual(trimmed(textRows(MARKS.caseLabels[1])));
    expect(text.px).toBeGreaterThanOrEqual(0.001 - 1e-9);
    for (const q of line.flatMap(local)) {
      expect(q[1]).toBeGreaterThanOrEqual(sticker.lo[1]);
      expect(q[1]).toBeLessThanOrEqual(sticker.hi[1]);
      expect(q[2]).toBeGreaterThanOrEqual(sticker.lo[2]);
      expect(q[2]).toBeLessThanOrEqual(sticker.hi[2]);
    }
  });

  it("bands the lid seam about 0.17 high", () => {
    const seam = parts
      .filter((p) => p.method === "box")
      .map(localShape)
      .filter((s) => s.lo[2] > 0.16 && s.hi[2] < 0.18);
    expect(seam).toHaveLength(4);
    for (const s of seam) {
      expect(s.hi[2] - s.lo[2]).toBeCloseTo(0.007, 3);
      expect((s.lo[2] + s.hi[2]) / 2).toBeCloseTo(0.17, 2);
    }
  });

  it("wraps the red stripe round all four faces", () => {
    const { hw, hd } = curioHalf("fuel-case", 0);
    const stripe = parts
      .filter((p) => p.method === "box")
      .map(localShape)
      .filter(
        (s) =>
          Math.abs(s.lo[2] - 0.07) < 1e-6 && Math.abs(s.hi[2] - 0.1) < 1e-6,
      );
    expect(stripe).toHaveLength(4);
    expect(stripe.some((s) => s.hi[0] >= hw - 0.03)).toBe(true);
    expect(stripe.some((s) => s.lo[0] <= -hw + 0.03)).toBe(true);
    expect(stripe.some((s) => s.hi[1] >= hd - 0.03)).toBe(true);
    expect(stripe.some((s) => s.lo[1] <= -hd + 0.03)).toBe(true);
  });
});
