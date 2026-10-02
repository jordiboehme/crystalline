import { describe, expect, it } from "vitest";

import { HERO_FOOTING, heroFootprint, heroLift } from "../world/footprints";
import {
  HERO_CATALOGUE,
  HERO_FLOOR_TOP,
  HERO_KINDS,
  HERO_WALL_TOP,
  heroSurfaces,
} from "../world/heroes";
import { turnForSide, wallAnchor } from "../world/sites";
import type { Hero, HeroKind, Side, WallSlot } from "../world/types";
import { CELL } from "../world/units";
import { FLAG, createBuilder, type MeshData, type V3 } from "./geometry";
import { createKit, frameAt, frameForSlot, type Frame } from "./kit";
import { LOOK } from "./looks";
import {
  clearAbove,
  floatingGlow,
  inBox,
  looseParts,
  placeMesh,
  placeParts,
  positions,
  recordingKitAt,
  upwardFaceAt,
  worstWinding,
  type Part,
} from "./modelChecks";
import { buildHero, buildHeroMesh } from "./models/heroes";
import { boxLeafMovers } from "./models/heroes/street";
import { HERO_BANK } from "./models/heroes/common";
import { FLOOR_TOP, WALL_TOP } from "./models/props/common";

const EPS = 1e-4;
/**
 * A free hero's anchor, in cell units: the middle of a cell's width, on a row
 * line.
 */
const FREE_AT = { x: 4.5, y: 3 } as const;
const SIDES: readonly Side[] = ["n", "e", "s", "w"];

/** The grid a catalogue surface is sampled on, in each direction. */
const SURFACE_GRID = 7;

/**
 * How high above a catalogue surface must stay clear of mesh, in metres:
 * enough room for a hand or a held object. The laser desk's arm passes
 * well above this over its desk top, so the rule holds there too; a kind
 * that turns out to need less is a finding for the report, not a reason to
 * shrink this number.
 */
const HEADROOM = 0.25;

/** The wall edge whose side takes turn `t`. */
function sideFor(t: number): Side {
  const side = SIDES.find((s) => turnForSide(s) === t);
  if (!side) throw new Error(`no side for turn ${String(t)}`);
  return side;
}

/** A hero built once at the origin, with every kit call recorded. */
function buildRecorded(
  kind: HeroKind,
  variant: number,
): { mesh: MeshData; parts: Part[] } {
  const builder = createBuilder();
  const parts: Part[] = [];
  buildHero(recordingKitAt(builder, parts), kind, variant, LOOK);
  return { mesh: builder.build(), parts };
}

/**
 * The hero at turn `t`: centred on `FREE_AT` for a free footing, on the
 * wall point of the edge at `(3, 4)` whose side takes the turn for a
 * wall-anchored one; its world offset in metres, and that edge.
 */
function heroAt(
  kind: HeroKind,
  variant: number,
  t: number,
): { hero: Hero; at: V3; edge: WallSlot | null } {
  if (HERO_FOOTING[kind] === "free") {
    return {
      hero: { kind, variant, x: FREE_AT.x, y: FREE_AT.y, turn: t, seed: 0 },
      at: [FREE_AT.x * CELL, 0, FREE_AT.y * CELL],
      edge: null,
    };
  }
  const edge: WallSlot = { x: 3, y: 4, side: sideFor(t) };
  const a = wallAnchor(edge);
  return {
    hero: { kind, variant, x: a.x, y: a.y, turn: t, seed: 0 },
    at: [a.x * CELL, 0, a.y * CELL],
    edge,
  };
}

/** Triangles per kind and variant at turn 0, for the table at the end. */
const triangles: string[] = [];

describe("hero models", () => {
  for (const kind of HERO_KINDS) {
    const entry = HERO_CATALOGUE[kind];
    for (let v = 0; v < entry.variants; v++) {
      const { mesh, parts } = buildRecorded(kind, v);
      triangles.push(`${kind} ${String(v)}: ${String(mesh.count / 3)}`);

      it(`${kind} variant ${String(v)} builds the same floats twice`, () => {
        // Built as the checks build it, moving parts in place.
        const b = createBuilder();
        buildHero((f) => createKit(b, f), kind, v, LOOK);
        const again = b.build();
        expect(again.count).toBe(mesh.count);
        expect(Array.from(again.vertices)).toEqual(Array.from(mesh.vertices));
      });

      it(`${kind} variant ${String(v)} stands on the floor, its wall or another part`, () => {
        const wall =
          HERO_FOOTING[kind] === "free" ? null : frameAt([0, 0, 0], 0);
        expect(looseParts(parts, wall, heroLift(kind))).toEqual([]);
      });

      if (HERO_FOOTING[kind] !== "flush")
        it(`${kind} variant ${String(v)} rests exactly at its lift`, () => {
          // Mutation caught: a floater whose part hangs below its lift, or
          // a standing hero that floats clear of the floor.
          const low = Math.min(...positions(mesh).map((p) => p[1]));
          expect(low).toBeCloseTo(heroLift(kind), 4);
        });

      for (let t = 0; t < 4; t++) {
        describe(`${kind} variant ${String(v)} turned ${String(t)}`, () => {
          const { hero, at, edge } = heroAt(kind, v, t);
          const placed = placeMesh(mesh, t, at);
          const wall: Frame | null = edge === null ? null : frameForSlot(edge);

          it("stays inside its footprint and under its top", () => {
            const box = heroFootprint(hero);
            const points = positions(placed);
            expect(points.length).toBeGreaterThan(0);
            for (const p of points) {
              expect(inBox(box, p, EPS)).toBe(true);
              expect(p[1]).toBeGreaterThanOrEqual(heroLift(kind) - EPS);
              expect(p[1]).toBeLessThanOrEqual(entry.top + EPS);
            }
          });

          it("reaches its top", () => {
            const top = Math.max(...positions(placed).map((p) => p[1]));
            expect(top).toBeGreaterThanOrEqual(entry.top - 0.15);
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

          if (entry.surfaces.length > 0) {
            it("puts every catalogue surface on a real, clear face of the mesh", () => {
              const tops = heroSurfaces(hero);
              expect(tops.length).toBeGreaterThan(0);
              for (const surf of tops) {
                for (let ix = 0; ix < SURFACE_GRID; ix++)
                  for (let iz = 0; iz < SURFACE_GRID; iz++) {
                    const x =
                      surf.box.x0 +
                      ((surf.box.x1 - surf.box.x0) * ix) / (SURFACE_GRID - 1);
                    const z =
                      surf.box.z0 +
                      ((surf.box.z1 - surf.box.z0) * iz) / (SURFACE_GRID - 1);
                    const label = `${kind} v${String(v)} t${String(t)} (${String(ix)},${String(iz)})`;
                    expect(upwardFaceAt(placed, x, z, surf.h), label).toBe(
                      true,
                    );
                    expect(
                      clearAbove(placed, x, z, surf.h, HEADROOM),
                      label,
                    ).toBe(true);
                  }
              }
            });
          }
        });
      }
    }
  }

  it("gives the world the models' wall top", () => {
    expect(HERO_WALL_TOP).toBe(WALL_TOP);
  });

  it("gives the world the props' floor top", () => {
    expect(HERO_FLOOR_TOP).toBe(FLOOR_TOP);
  });

  it("puts blink-flagged parts exactly in the kinds whose bank blinks", () => {
    for (const kind of HERO_KINDS)
      for (let v = 0; v < HERO_CATALOGUE[kind].variants; v++) {
        const { parts } = buildRecorded(kind, v);
        const blinks = parts.some((p) => p.flag >= FLAG.blink);
        expect(blinks, `${kind} ${String(v)}`).toBe(
          HERO_BANK[kind] !== "steady",
        );
      }
  });

  it("checks the police box with its door leaves in place (2.6e C19)", () => {
    // Mutation caught: buildHero's default turned to leave the moving parts
    // out, so the loop above would check a doorless police box's envelope,
    // winding, glow contact, float and budget and never the leaves'.
    const { mesh } = buildRecorded("police-box", 0);
    const explicit = createBuilder();
    buildHero(recordingKitAt(explicit, []), "police-box", 0, LOOK, true);
    const body = buildHeroMesh("police-box", 0, LOOK);
    const wings = boxLeafMovers(
      { kind: "police-box", variant: 0, x: 3, y: 4, turn: 0, seed: 0 },
      0,
      LOOK,
    );
    expect(mesh.count).toBe(explicit.build().count);
    expect(mesh.count).toBe(
      body.count + wings.reduce((n, m) => n + m.mesh.count, 0),
    );
    expect(mesh.count).toBeGreaterThan(body.count);
    expect(mesh.count / 3).toBeLessThan(4000);
  });

  it("refuses a variant the catalogue does not have", () => {
    expect(() => buildHeroMesh("turret", 1, LOOK)).toThrow(/no variant 1/);
    expect(() => buildHeroMesh("arcade-cabinet", -1, LOOK)).toThrow(
      /no variant -1/,
    );
  });

  it("logs every kind and variant's triangle count at turn 0", () => {
    expect(triangles).toHaveLength(
      HERO_KINDS.reduce((n, k) => n + HERO_CATALOGUE[k].variants, 0),
    );
    console.info(`hero triangles at turn 0:\n${triangles.join("\n")}`);
  });
});
