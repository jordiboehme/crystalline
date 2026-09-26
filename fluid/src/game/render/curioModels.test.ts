/**
 * The curio models' shared checks (2.6b), modelled on the hero test: every
 * kind and variant is built once at the origin, placed at every turn at an
 * instance height of 0.9 m the way the GPU places it, and measured. It
 * stays inside its turned catalogue size and between its surface and its
 * top, reaches that top, sits on its surface (a hovering curio's lowest
 * vertex is its lift over it, `curioLift`), winds every triangle with
 * its normal, stays under the triangle budget, glows only on its body, has
 * no part floating clear of its base or another part (held from its
 * lift), and carries blink
 * parts exactly when its kind's bank (`CURIO_BANK`) blinks. What only a
 * kind's own recipe carries is the per-batch tests' job.
 */

import { describe, expect, it } from "vitest";

import {
  CURIO_CATALOGUE,
  CURIO_KINDS,
  curioBox,
  curioLift,
} from "../world/curios";
import type { Curio, CurioKind } from "../world/types";
import { CELL } from "../world/units";
import { FLAG, createBuilder, type MeshData, type V3 } from "./geometry";
import { LOOKS } from "./looks";
import {
  floatingGlow,
  inBox,
  looseParts,
  placeMesh,
  placeParts,
  positions,
  recordingKitAt,
  worstWinding,
  type Part,
} from "./modelChecks";
import { buildCurio, buildCurioMesh } from "./models/curios";
import { CURIO_BANK, curioHalf } from "./models/curios/common";

const EPS = 1e-4;
/** The instance height every curio is placed at: a surface 0.9 m up. */
const H = 0.9;
/** Where every curio is placed, in cell units. */
const AT = { x: 4.5, y: 3 } as const;
/** The triangle cap a curio must stay under. */
const BUDGET = 1500;

/** A curio built once at the origin, with every kit call recorded. */
function buildRecorded(
  kind: CurioKind,
  variant: number,
): { mesh: MeshData; parts: Part[] } {
  const builder = createBuilder();
  const parts: Part[] = [];
  buildCurio(recordingKitAt(builder, parts), kind, variant, LOOKS.aperture);
  return { mesh: builder.build(), parts };
}

/** Triangles per kind and variant at turn 0, for the table at the end. */
const triangles: string[] = [];

describe("curio models", () => {
  for (const kind of CURIO_KINDS) {
    const entry = CURIO_CATALOGUE[kind];
    for (let v = 0; v < entry.variants; v++) {
      const { mesh, parts } = buildRecorded(kind, v);
      const { top } = curioHalf(kind, v);
      triangles.push(`${kind} ${String(v)}: ${String(mesh.count / 3)}`);

      it(`${kind} variant ${String(v)} builds the same floats twice`, () => {
        const again = buildCurioMesh(kind, v, LOOKS.aperture);
        expect(again.count).toBe(mesh.count);
        expect(Array.from(again.vertices)).toEqual(Array.from(mesh.vertices));
      });

      it(`${kind} variant ${String(v)} rests on its base or another part, from its lift`, () => {
        // Mutation caught: the drone's lift ignored here, so its whole
        // shell counts as floating.
        expect(looseParts(parts, null, curioLift(kind))).toEqual([]);
      });

      it(`${kind} variant ${String(v)} blinks exactly when its bank does`, () => {
        const blinks = parts.some((p) => p.flag >= FLAG.blink);
        expect(blinks).toBe(CURIO_BANK[kind] !== "steady");
      });

      for (let t = 0; t < 4; t++) {
        describe(`${kind} variant ${String(v)} turned ${String(t)}`, () => {
          const at: V3 = [AT.x * CELL, H, AT.y * CELL];
          const placed = placeMesh(mesh, t, at);
          const curio: Curio = {
            kind,
            variant: v,
            x: AT.x,
            y: AT.y,
            h: H,
            turn: t,
            seed: 0,
          };

          it("stays inside its turned size and between its surface and its top", () => {
            const box = curioBox(curio);
            const points = positions(placed);
            expect(points.length).toBeGreaterThan(0);
            for (const p of points) {
              expect(inBox(box, p, EPS)).toBe(true);
              expect(p[1]).toBeGreaterThanOrEqual(H - EPS);
              expect(p[1]).toBeLessThanOrEqual(H + top + EPS);
            }
          });

          it("reaches its top, and its lowest vertex is its lift over its surface", () => {
            // Mutation caught: a curio that floats clear of its surface, or
            // a drone part hanging below its lift.
            const ys = positions(placed).map((p) => p[1]);
            expect(Math.max(...ys)).toBeGreaterThanOrEqual(H + top - 1e-3);
            expect(
              Math.abs(Math.min(...ys) - (H + curioLift(kind))),
            ).toBeLessThanOrEqual(EPS);
          });

          it("winds every triangle with its normal", () => {
            expect(placed.count).toBeGreaterThan(0);
            expect(placed.count % 3).toBe(0);
            expect(worstWinding(placed)).toBeGreaterThan(0.999);
          });

          it("stays under the triangle budget", () => {
            expect(placed.count / 3).toBeLessThan(BUDGET);
          });

          it("glows only on or in its body", () => {
            expect(floatingGlow(placeParts(parts, t, at), null)).toEqual([]);
          });
        });
      }
    }
  }

  it("refuses a variant the catalogue does not have", () => {
    expect(() => buildCurioMesh("green-pistol", 1, LOOKS.aperture)).toThrow(
      /no variant 1/,
    );
    expect(() => buildCurioMesh("light-sword", -1, LOOKS.aperture)).toThrow(
      /no variant -1/,
    );
  });

  it("logs every kind and variant's triangle count at turn 0", () => {
    expect(triangles).toHaveLength(
      CURIO_KINDS.reduce((n, k) => n + CURIO_CATALOGUE[k].variants, 0),
    );
    console.info(`curio triangles at turn 0:\n${triangles.join("\n")}`);
  });
});
