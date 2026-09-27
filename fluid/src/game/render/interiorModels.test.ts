/**
 * The console room's fittings as models (2.6e C2, C19): every kind and
 * variant, built at the origin and placed as the GPU places an instance at
 * every turn it can take, against the family's rules. The blockouts pass
 * it; the finished recipes keep it green.
 */

import { describe, expect, it } from "vitest";

import { INTERIOR_CATALOGUE } from "../world/consoleRoom";
import { turnedBox } from "../world/footprints";
import { turnForSide, wallAnchor } from "../world/sites";
import type {
  InteriorKind,
  InteriorPiece,
  Side,
  WallSlot,
} from "../world/types";
import { CELL } from "../world/units";
import { FLAG, createBuilder, type MeshData, type V3 } from "./geometry";
import { frameAt, frameForSlot, type Frame } from "./kit";
import { LOOKS, type Look } from "./looks";
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
import { buildInterior, buildInteriorMesh } from "./models/interior";
import { INTERIOR_BANK, interiorHalf } from "./models/interior/common";

const EPS = 1e-4;

/** Every fitting kind, in the catalogue's order. */
const KINDS = Object.keys(INTERIOR_CATALOGUE) as InteriorKind[];

/** A free fitting's anchor, in cell units, as the console room's centre. */
const FREE_AT = { x: 3, y: 3 } as const;

const SIDES: readonly Side[] = ["n", "e", "s", "w"];

/**
 * Each kind's triangle budget (C19): a wall of roundels and the scanner
 * under 1500, the inner doors under 3000, the console with its column's
 * frame under 4000.
 */
const BUDGET = {
  "roundel-wall": 1500,
  "inner-doors": 3000,
  scanner: 1500,
  console: 4000,
} as const satisfies Record<InteriorKind, number>;

/** The wall edge whose side takes turn `t`. */
function sideFor(t: number): Side {
  const side = SIDES.find((s) => turnForSide(s) === t);
  if (!side) throw new Error(`no side for turn ${String(t)}`);
  return side;
}

/** A fitting built once at the origin, with every kit call recorded. */
function buildRecorded(
  kind: InteriorKind,
  variant: number,
): { mesh: MeshData; parts: Part[] } {
  const builder = createBuilder();
  const parts: Part[] = [];
  buildInterior(recordingKitAt(builder, parts), kind, variant, LOOKS.aperture);
  return { mesh: builder.build(), parts };
}

/**
 * The turns a kind can take: the four sides for a flush piece, turn 0 for
 * the console, which stands only at the room's centre.
 */
const turnsOf = (kind: InteriorKind): number[] =>
  INTERIOR_CATALOGUE[kind].footing === "flush" ? [0, 1, 2, 3] : [0];

/**
 * The fitting at turn `t`: centred on `FREE_AT` for the console, on the
 * wall point of the edge at `(3, 4)` whose side takes the turn for a flush
 * piece; its world offset in metres, and that edge.
 */
function pieceAt(
  kind: InteriorKind,
  variant: number,
  t: number,
): { piece: InteriorPiece; at: V3; edge: WallSlot | null } {
  if (INTERIOR_CATALOGUE[kind].footing === "free") {
    return {
      piece: { kind, variant, x: FREE_AT.x, y: FREE_AT.y, turn: t, seed: 0 },
      at: [FREE_AT.x * CELL, 0, FREE_AT.y * CELL],
      edge: null,
    };
  }
  const edge: WallSlot = { x: 3, y: 4, side: sideFor(t) };
  const a = wallAnchor(edge);
  return {
    piece: { kind, variant, x: a.x, y: a.y, turn: t, seed: 0 },
    at: [a.x * CELL, 0, a.y * CELL],
    edge,
  };
}

/** Triangles per kind and variant, for the table at the end. */
const triangles: string[] = [];

describe("interior models (2.6e C2)", () => {
  it("has the four kinds", () => {
    // Mutation caught: a kind dropped from the catalogue, which every loop
    // below would then skip.
    expect(KINDS).toEqual([
      "roundel-wall",
      "inner-doors",
      "scanner",
      "console",
    ]);
  });

  for (const kind of KINDS) {
    const entry = INTERIOR_CATALOGUE[kind];
    for (let v = 0; v < entry.variants; v++) {
      const { mesh, parts } = buildRecorded(kind, v);
      triangles.push(`${kind} ${String(v)}: ${String(mesh.count / 3)}`);

      it(`${kind} variant ${String(v)} builds the same floats as its instanced mesh`, () => {
        // Mutation caught: a recipe that reads anything but its kind,
        // variant and look, or an instanced mesh that is not the model.
        const again = buildInteriorMesh(kind, v, LOOKS.aperture);
        expect(again.count).toBe(mesh.count);
        expect(Array.from(again.vertices)).toEqual(Array.from(mesh.vertices));
      });

      it(`${kind} variant ${String(v)} stands on the floor or its wall`, () => {
        // Mutation caught: a part hung in mid-air, clear of the floor, the
        // wall and every other part.
        expect(parts.length).toBeGreaterThan(0);
        const wall = entry.footing === "flush" ? frameAt([0, 0, 0], 0) : null;
        expect(looseParts(parts, wall)).toEqual([]);
      });

      it(`${kind} variant ${String(v)} stays under its triangle budget (C19)`, () => {
        // Mutation caught: a recipe grown past its share of the room.
        expect(mesh.count / 3).toBeLessThan(BUDGET[kind]);
      });

      for (const t of turnsOf(kind)) {
        describe(`${kind} variant ${String(v)} turned ${String(t)}`, () => {
          const { piece, at, edge } = pieceAt(kind, v, t);
          const placed = placeMesh(mesh, t, at);
          const wall: Frame | null = edge === null ? null : frameForSlot(edge);

          it("stays inside its envelope and under its top", () => {
            // Mutation caught: a part out past the piece's width, behind
            // its wall, out past its depth or over its top.
            const { hw, d0, d1, top } = interiorHalf(kind);
            const box = turnedBox(piece.x, piece.y, piece.turn, {
              a0: -hw,
              a1: hw,
              d0,
              d1,
            });
            const points = positions(placed);
            expect(points.length).toBeGreaterThan(0);
            for (const p of points) {
              expect(inBox(box, p, EPS)).toBe(true);
              expect(p[1]).toBeGreaterThanOrEqual(-EPS);
              expect(p[1]).toBeLessThanOrEqual(top + EPS);
            }
          });

          it("winds every triangle with its normal", () => {
            // Mutation caught: a face wound the wrong way, which the
            // renderer would cull.
            expect(placed.count).toBeGreaterThan(0);
            expect(placed.count % 3).toBe(0);
            expect(worstWinding(placed)).toBeGreaterThan(0.999);
          });

          it("glows only on or in its body", () => {
            // Mutation caught: a light hung clear of the piece.
            expect(floatingGlow(placeParts(parts, t, at), wall)).toEqual([]);
          });
        });
      }
    }
  }

  it("puts blink-flagged parts only in the kinds whose bank blinks, and some in each of those", () => {
    // Mutation caught: a steady kind (the inner doors, the scanner) given a
    // blinking light, which its slot would hold still, or a blinking kind
    // with no light for its bank to move. Checked per kind, not per
    // variant: a wall of roundels without a glowing one is a variant of a
    // blinking kind (C5).
    expect(KINDS.length).toBe(4);
    for (const kind of KINDS) {
      const variants = Array.from(
        { length: INTERIOR_CATALOGUE[kind].variants },
        (_, v) =>
          buildRecorded(kind, v).parts.some((p) => p.flag >= FLAG.blink),
      );
      if (INTERIOR_BANK[kind] === "steady")
        expect(variants.some(Boolean), kind).toBe(false);
      else expect(variants.some(Boolean), kind).toBe(true);
    }
  });

  it("keeps the wall pieces' colours in every look (C4)", () => {
    // Mutation caught: a wall piece painted with a look's colour
    // (`s.body`, `s.panel`, `s.dark`), which would turn the room's white
    // beige in one look and grey in another.
    const walls = KINDS.filter((k) => k !== "console");
    expect(walls.length).toBe(3);
    expect(Object.values(LOOKS).length).toBeGreaterThan(1);
    for (const kind of walls)
      for (let v = 0; v < INTERIOR_CATALOGUE[kind].variants; v++) {
        const tintsIn = (look: Look) => {
          const parts: Part[] = [];
          buildInterior(recordingKitAt(createBuilder(), parts), kind, v, look);
          return parts.map((p) => p.tint?.join() ?? "none");
        };
        const first = tintsIn(LOOKS.aperture);
        expect(first.length).toBeGreaterThan(0);
        for (const look of Object.values(LOOKS))
          expect(tintsIn(look), `${kind} ${String(v)} ${look.id}`).toEqual(
            first,
          );
      }
  });

  it("refuses a variant the catalogue does not have", () => {
    // Mutation caught: the variant check dropped from `buildInterior`.
    expect(() => buildInteriorMesh("console", 1, LOOKS.aperture)).toThrow(
      /no variant 1/,
    );
    expect(() => buildInteriorMesh("roundel-wall", -1, LOOKS.aperture)).toThrow(
      /no variant -1/,
    );
  });

  it("logs every kind and variant's triangle count", () => {
    // Mutation caught: a kind or a variant the loop above skipped.
    expect(triangles).toHaveLength(
      KINDS.reduce((n, k) => n + INTERIOR_CATALOGUE[k].variants, 0),
    );
    console.info(`interior triangles:\n${triangles.join("\n")}`);
  });
});
