/**
 * The console's shape tests (2.6e C8, C24d): a six-sided desk whose six
 * panels slope from the centre ring down to the rim at waist height, each
 * carrying switches, two dials, a lever and a row of four lights that
 * blink out of step with the other panels', and the column's open frame
 * of six ribs and two rings round the rotor. The console is built at the
 * origin with each kit call recorded; the rotor is built as its mover.
 */

import { describe, expect, it } from "vitest";

import { INTERIOR_CATALOGUE, consoleRoom } from "../../../world/consoleRoom";
import type { InteriorPiece } from "../../../world/types";
import { CELL } from "../../../world/units";
import {
  FLAG,
  FLOATS_PER_VERTEX,
  createBuilder,
  type V3,
} from "../../geometry";
import { LOOKS } from "../../looks";
import {
  positions,
  recordingKitAt,
  shape,
  touching,
  type Part,
} from "../../modelChecks";
import { buildInterior, buildInteriorMovers } from ".";
import {
  COLUMN,
  CONSOLE,
  CONSOLE_PANEL,
  ROTOR,
  ROTOR_TRAVEL,
  rotorMover,
} from "./console";

/** The console built at the origin with every kit call recorded. */
function consoleParts(): Part[] {
  const parts: Part[] = [];
  buildInterior(
    recordingKitAt(createBuilder(), parts),
    "console",
    0,
    LOOKS.aperture,
  );
  return parts;
}
function boundsOf(p: Part): { lo: V3; hi: V3; mid: V3 } {
  const s = shape(p.points);
  return {
    lo: s.lo,
    hi: s.hi,
    mid: [
      (s.lo[0] + s.hi[0]) / 2,
      (s.lo[1] + s.hi[1]) / 2,
      (s.lo[2] + s.hi[2]) / 2,
    ],
  };
}
const height = (p: Part) => boundsOf(p).hi[1] - boundsOf(p).lo[1];
const radial = (q: V3) => Math.hypot(q[0], q[2]);
/** A desk panel: `CONSOLE_PANEL` tint, reaching from the ring out to the rim. */
const isPanel = (p: Part) =>
  p.tint !== null &&
  p.tint.every((c, i) => Math.abs(c - (CONSOLE_PANEL[i] ?? -1)) < 1e-6) &&
  height(p) > (CONSOLE.ring - CONSOLE.rim) / 2;
/** Inside the column's radius and between its rings. */
const inColumn = (p: Part) =>
  p.points.every(
    (q) =>
      radial(q) <= COLUMN.radius + 0.03 &&
      q[1] >= COLUMN.h0 - 0.01 &&
      q[1] <= COLUMN.h1 + 0.01,
  );
/** A rib: thin in plan, nearly the column's height. */
const isRib = (p: Part) => {
  const b = boundsOf(p);
  return (
    Math.max(b.hi[0] - b.lo[0], b.hi[2] - b.lo[2]) < 0.05 &&
    height(p) > (COLUMN.h1 - COLUMN.h0) * 0.8
  );
};
/** A part that goes all the way round the column's axis (a ring or a pane). */
const wraps = (p: Part) => {
  const b = boundsOf(p);
  return (
    b.lo[0] < -COLUMN.radius * 0.8 &&
    b.hi[0] > COLUMN.radius * 0.8 &&
    b.lo[2] < -COLUMN.radius * 0.8 &&
    b.hi[2] > COLUMN.radius * 0.8
  );
};

/** The console room's console, as the room places it. */
function consolePiece(): { piece: InteriorPiece; index: number } {
  const pieces = consoleRoom().interior ?? [];
  const index = pieces.findIndex((p) => p.kind === "console");
  const piece = pieces[index];
  if (!piece) throw new Error("the console room has no console");
  return { piece, index };
}

describe("the console (2.6e C8)", () => {
  it("slopes six panels down from the centre ring to the rim at waist height (2.6e C8)", () => {
    // Mutation caught: flat panels, five or seven sides, or the rim off waist height.
    const panels = consoleParts().filter(isPanel);
    expect(panels.length).toBe(6);
    const bearings = panels
      .map((p) => {
        const b = boundsOf(p);
        expect(b.hi[1]).toBeCloseTo(CONSOLE.ring, 2);
        expect(b.lo[1]).toBeCloseTo(CONSOLE.rim, 2);
        return (
          Math.round((Math.atan2(b.mid[2], b.mid[0]) * 180) / Math.PI + 360) %
          360
        );
      })
      .sort((x, y) => x - y);
    bearings.forEach((d, i) => {
      if (i > 0) expect(d - (bearings[i - 1] ?? 0)).toBeCloseTo(60, -1);
    });
  });

  it("puts switches, two dials, a lever and four blinking lights on every panel", () => {
    // Mutation caught: a bare panel, or the lights all in one group (in step).
    const parts = consoleParts();
    const panels = parts.filter(isPanel);
    expect(panels.length).toBe(6);
    const groups = new Set<number>();
    for (const panel of panels) {
      const on = parts.filter(
        (p) =>
          p !== panel &&
          touching(shape(p.points), shape(panel.points)) &&
          !isPanel(p),
      );
      const lights = on.filter((p) => p.flag >= FLAG.blink);
      for (const l of lights) groups.add(l.flag - FLAG.blink);
      expect(lights.length).toBe(4);
      expect(
        on.filter((p) => p.method === "cylinder" && p.flag < FLAG.blink).length,
      ).toBeGreaterThanOrEqual(2);
      expect(
        on.filter((p) => p.method === "box" && p.flag < FLAG.blink).length,
      ).toBeGreaterThanOrEqual(4); // three switches and a lever at least
    }
    expect(groups.size).toBe(8);
  });

  it("frames the column with six ribs and two rings and no pane, so the rotor shows (2.6e C8)", () => {
    // Mutation caught: a solid pane round the column (it would hide the rotor).
    const col = consoleParts().filter(inColumn);
    expect(col.length).toBeGreaterThan(0);
    expect(col.filter(isRib).length).toBe(COLUMN.ribs);
    expect(col.some((p) => wraps(p) && height(p) > 0.5)).toBe(false);
  });

  it("widens the column and the rotor to the research note's sizes (C24d)", () => {
    // Mutation caught: the plan's first guesses (a 0.28 m column, a 0.2 m
    // rotor) left in place of the note's 0.38 m and 0.27 m.
    expect(COLUMN.radius).toBe(0.38);
    expect(ROTOR.radius).toBe(0.27);
    const ribs = consoleParts().filter(inColumn).filter(isRib);
    expect(ribs.length).toBe(COLUMN.ribs);
    for (const rib of ribs)
      expect(radial(boundsOf(rib).mid)).toBeGreaterThan(ROTOR.radius + 0.05);
  });
});

describe("the console's collision box (2.6e C8)", () => {
  it("is the desk's size and the column's height, as the catalogue gives it", () => {
    // Mutation caught: the catalogue's console box (its collision
    // footprint and envelope) left at one size while the model is built at
    // another.
    const entry = INTERIOR_CATALOGUE.console;
    expect(entry.width).toBe(CONSOLE.corners);
    expect(entry.depth).toBe(CONSOLE.corners);
    expect(entry.top).toBe(COLUMN.h1);
  });
});

describe("the rotor (2.6e C8, C9)", () => {
  it("is the console's one mover, a slide straight up keyed by the piece", () => {
    // Mutation caught: a rotor keyed like a door, turned about a pivot,
    // sliding sideways, tied to a fixture's fault frame, or built for a
    // wall piece as well as the console.
    const { piece, index } = consolePiece();
    const m = rotorMover(piece, index, LOOKS.aperture);
    expect(m.key).toBe(`rotor:${String(index)}`);
    expect(m.part).toBe("rotor");
    expect(m.fixture).toBe(-1);
    expect(m.axis).toEqual([0, 1, 0]);
    expect(m.travel).toBe(ROTOR_TRAVEL);
    expect(m.pivot).toBeNull();
    expect(m.rest).toBe(1);
    expect(m.swing).toBe(0);
    expect(buildInteriorMovers(piece, index, LOOKS.aperture)).toHaveLength(1);
    const wall = (consoleRoom().interior ?? []).find(
      (p) => p.kind !== "console",
    );
    expect(wall).toBeDefined();
    if (wall) expect(buildInteriorMovers(wall, 0, LOOKS.aperture)).toEqual([]);
  });

  it("stands at rest on the column's base ring, inside the column, glowing", () => {
    // Mutation caught: a rotor built at the origin instead of the
    // console's anchor, one out past the ribs, or discs that do not glow.
    const { piece, index } = consolePiece();
    const m = rotorMover(piece, index, LOOKS.aperture);
    const pts = positions(m.mesh);
    expect(pts.length).toBeGreaterThan(0);
    const cx = piece.x * CELL;
    const cz = piece.y * CELL;
    const ys = pts.map((p) => p[1]);
    expect(Math.min(...ys)).toBeCloseTo(ROTOR.h0, 6);
    expect(Math.max(...ys)).toBeCloseTo(ROTOR.h1, 6);
    const r = Math.max(...pts.map((p) => Math.hypot(p[0] - cx, p[2] - cz)));
    expect(r).toBeLessThanOrEqual(ROTOR.radius + 1e-6);
    expect(r).toBeGreaterThan(ROTOR.radius - 0.01);
    const flags = new Set(
      Array.from(
        { length: m.mesh.count },
        (_, i) => m.mesh.vertices[i * FLOATS_PER_VERTEX + 12],
      ),
    );
    expect(flags.has(FLAG.signal)).toBe(true);
  });
});
