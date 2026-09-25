/**
 * The keepsake batch's own shape tests: what `curioModels.test.ts`'s
 * generic checks do not pin for the star ball, the catch ball, the trap
 * box and the fuel case. Parts are found by method, layer and shape, in
 * the recipe's own local `(a, d, h)` terms (`toLocal`), never by colour:
 * a recorded part carries no tint, only its layer and its lighting flag.
 */

import { describe, expect, it } from "vitest";

import type { CurioKind } from "../../../world/types";
import { createBuilder } from "../../geometry";
import { frameAt, type Frame } from "../../kit";
import { LAYER } from "../../layers";
import { LOOKS } from "../../looks";
import {
  recordingKitAt,
  shape,
  toLocal,
  type Part,
  type Shape,
} from "../../modelChecks";
import { curioHalf } from "./common";
import { buildCurio } from "./index";

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
  it("carries three trefoil sectors and a centre disc on the label", () => {
    const shapes = partsOf("fuel-case")
      .filter((p) => p.method === "extrude")
      .map(localShape);
    expect(shapes).toHaveLength(4);
    const diagonal = (s: Shape) =>
      Math.hypot(s.hi[0] - s.lo[0], s.hi[2] - s.lo[2]);
    const sectors = shapes.filter((s) => diagonal(s) > 0.025);
    const disc = shapes.filter((s) => diagonal(s) <= 0.025);
    expect(sectors).toHaveLength(3);
    expect(disc).toHaveLength(1);
  });

  it("wraps the red stripe round all four faces", () => {
    const { hw, hd } = curioHalf("fuel-case", 0);
    const stripe = partsOf("fuel-case")
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
