import { describe, expect, it } from "vitest";

import { propFootprint } from "../world/footprints";
import { PROP_CATALOGUE, PROP_KINDS } from "../world/props";
import { turnForSide, wallAnchor } from "../world/sites";
import type { PropKind, Side } from "../world/types";
import { CELL } from "../world/units";
import {
  FLOATS_PER_VERTEX,
  createBuilder,
  type MeshData,
  type V3,
} from "./geometry";
import { frameForSlot, turnPoint, type Frame } from "./kit";
import { LOOKS } from "./looks";
import {
  floatingGlow,
  inBox,
  positions,
  recordingKitAt,
  toLocal,
  worstWinding,
  type Part,
} from "./modelChecks";
import { FLUSH_DEPTH, HEADROOM } from "./models";
import {
  CEILING_DROP,
  CEILING_OUT,
  CEILING_SETBACK,
  FLOOR_TOP,
  RUN_BAND,
  RUN_REACH,
  WALL_REACH,
  WALL_TOP,
} from "./models/props/common";
import { buildProp, buildPropMesh } from "./models/props";

const EPS = 1e-4;
/** The lowest ceiling the generator makes, where the ceiling anchor sits. */
const CEILING = 3.0;
/** The floor anchor, in cell units: the middle of a cell's width, on a row line. */
const FLOOR_AT = { x: 4.5, y: 3 } as const;
const SIDES: readonly Side[] = ["n", "e", "s", "w"];

/** The wall edge whose side takes turn `t`. */
function sideFor(t: number): Side {
  const side = SIDES.find((s) => turnForSide(s) === t);
  if (!side) throw new Error(`no side for turn ${String(t)}`);
  return side;
}

/** A prop built once at the origin, with every kit call recorded. */
function buildRecorded(
  kind: PropKind,
  variant: number,
): { mesh: MeshData; parts: Part[] } {
  const builder = createBuilder();
  const parts: Part[] = [];
  buildProp(recordingKitAt(builder, parts), kind, variant, {
    look: LOOKS.aperture,
  });
  return { mesh: builder.build(), parts };
}

/** Where an instance goes: its world offset, in metres. */
function anchorFor(kind: PropKind, t: number): V3 {
  const anchor = PROP_CATALOGUE[kind].anchor;
  if (anchor === "floor") return [FLOOR_AT.x * CELL, 0, FLOOR_AT.y * CELL];
  const a = wallAnchor({ x: 3, y: 4, side: sideFor(t) });
  return [a.x * CELL, anchor === "ceiling" ? CEILING : 0, a.y * CELL];
}

const add = (p: V3, q: V3): V3 => [p[0] + q[0], p[1] + q[1], p[2] + q[2]];

/**
 * A mesh turned and placed as the GPU places an instance: every position
 * turned by `turnPoint` and moved to the anchor, every normal turned.
 */
function placeMesh(m: MeshData, t: number, at: V3): MeshData {
  const vertices = Float32Array.from(m.vertices);
  for (let i = 0; i < m.count; i++) {
    const o = i * FLOATS_PER_VERTEX;
    const v = (k: number) => vertices[o + k] ?? NaN;
    const p = add(turnPoint([v(0), v(1), v(2)], t), at);
    const n = turnPoint([v(3), v(4), v(5)], t);
    vertices.set([...p, ...n], o);
  }
  return { vertices, count: m.count };
}

/** The recorded parts, turned and placed the same way. */
const placeParts = (parts: readonly Part[], t: number, at: V3): Part[] =>
  parts.map((p) => ({
    ...p,
    points: p.points.map((q) => add(turnPoint(q, t), at)),
  }));

describe("prop models", () => {
  for (const kind of PROP_KINDS) {
    const entry = PROP_CATALOGUE[kind];
    for (let v = 0; v < entry.variants; v++) {
      const { mesh, parts } = buildRecorded(kind, v);

      it(`${kind} variant ${String(v)} builds the same floats twice`, () => {
        const again = buildPropMesh(kind, v, LOOKS.aperture);
        expect(again.count).toBe(mesh.count);
        expect(Array.from(again.vertices)).toEqual(Array.from(mesh.vertices));
      });

      for (let t = 0; t < 4; t++) {
        describe(`${kind} variant ${String(v)} turned ${String(t)}`, () => {
          const at = anchorFor(kind, t);
          const placed = placeMesh(mesh, t, at);
          const wall: Frame | null =
            entry.anchor === "floor"
              ? null
              : frameForSlot({ x: 3, y: 4, side: sideFor(t) });

          it("stays inside its anchor's envelope", () => {
            const points = positions(placed);
            expect(points.length).toBeGreaterThan(0);
            if (entry.anchor === "floor") {
              const box = propFootprint({
                kind,
                variant: v,
                anchor: "floor",
                x: FLOOR_AT.x,
                y: FLOOR_AT.y,
                turn: t,
                seed: 0,
              });
              if (!box) throw new Error("no footprint");
              for (const p of points) {
                expect(inBox(box, p, EPS)).toBe(true);
                expect(p[1]).toBeGreaterThanOrEqual(-EPS);
                expect(p[1]).toBeLessThanOrEqual(FLOOR_TOP + EPS);
              }
              return;
            }
            if (!wall) throw new Error("no wall frame");
            const reach = entry.run ? RUN_REACH : WALL_REACH;
            for (const p of points) {
              const [a, d, h] = toLocal(wall, p);
              expect(Math.abs(a)).toBeLessThanOrEqual(reach + EPS);
              if (entry.anchor === "wall") {
                expect(d).toBeGreaterThanOrEqual(-EPS);
                expect(d).toBeLessThanOrEqual(FLUSH_DEPTH + EPS);
                const [h0, h1] = entry.run
                  ? [RUN_BAND.h0, RUN_BAND.h1]
                  : [0, WALL_TOP];
                expect(h).toBeGreaterThanOrEqual(h0 - EPS);
                expect(h).toBeLessThanOrEqual(h1 + EPS);
              } else {
                expect(d).toBeGreaterThanOrEqual(CEILING_SETBACK - EPS);
                expect(d).toBeLessThanOrEqual(CEILING_OUT + EPS);
                expect(h).toBeGreaterThanOrEqual(CEILING - CEILING_DROP - EPS);
                expect(h).toBeLessThanOrEqual(CEILING - HEADROOM + EPS);
              }
            }
          });

          it("winds every triangle with its normal", () => {
            expect(placed.count).toBeGreaterThan(0);
            expect(placed.count % 3).toBe(0);
            expect(worstWinding(placed)).toBeGreaterThan(0.999);
          });

          it("stays under the triangle budget", () => {
            expect(placed.count / 3).toBeLessThan(4000);
          });

          it("glows only on or in its body", () => {
            expect(floatingGlow(placeParts(parts, t, at), wall)).toEqual([]);
          });
        });
      }
    }
  }

  it("refuses a variant the catalogue does not have", () => {
    expect(() => buildPropMesh("crate", 3, LOOKS.aperture)).toThrow(
      /no variant 3/,
    );
    expect(() => buildPropMesh("duct", -1, LOOKS.aperture)).toThrow(
      /no variant -1/,
    );
  });
});
