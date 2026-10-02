/**
 * The station's set dressing: light grey bodies (`propBody`, `propLook`) on
 * the props, the machines, the terminals and the furniture, never on the
 * shell's fixtures, the heroes, the curios or the fittings.
 */

import { describe, expect, it } from "vitest";

import { CURIO_CATALOGUE, CURIO_KINDS } from "../world/curios";
import { INTERIOR_CATALOGUE } from "../world/consoleRoom";
import { HERO_CATALOGUE, HERO_KINDS } from "../world/heroes";
import type { Fixture, InteriorKind } from "../world/types";
import { FLOATS_PER_VERTEX, fixtureLook, type MeshData } from "./geometry";
import { LOOK, propLook, type Look, type Rgb } from "./looks";
import { buildCurioMesh } from "./models/curios";
import { buildHeroMesh } from "./models/heroes";
import { buildInteriorMesh } from "./models/interior";
import { buildPropMesh } from "./models/props";
import { buildGroupMesh } from "./renderer";

const INTERIOR_KINDS = Object.keys(INTERIOR_CATALOGUE) as InteriorKind[];

/** Whether any vertex of `m` has the tint `c`. */
function hasTint(m: MeshData, c: Rgb): boolean {
  for (let i = 0; i < m.count; i++) {
    const o = i * FLOATS_PER_VERTEX + 9;
    if (
      Math.abs((m.vertices[o] ?? NaN) - c[0]) < 1e-6 &&
      Math.abs((m.vertices[o + 1] ?? NaN) - c[1]) < 1e-6 &&
      Math.abs((m.vertices[o + 2] ?? NaN) - c[2]) < 1e-6
    )
      return true;
  }
  return false;
}

describe("the look's light grey set dressing", () => {
  it("keeps the grey in the look's settings, light but darker than its walls", () => {
    // Mutation caught: the grey taken off the look, a dark grey (the
    // prototype's), or one too light to stand off the walls in bright
    // light (the first tuning's #bdbdb8).
    const body = LOOK.propBody?.body;
    if (body === undefined) throw new Error("the look has no prop body");
    const lum = (c: Rgb) => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    expect(lum(body)).toBeGreaterThan(0.6);
    // Dark enough to stand off the walls in the bright hangar too.
    expect(lum(body)).toBeLessThan(0.72);
    expect(lum(body)).toBeLessThan(lum(LOOK.palette.panel) - 0.15);
  });

  it("paints the props' bodies grey", () => {
    // Mutation caught: `propLook` dropped from the prop family (the props
    // back in the look's white).
    const body = LOOK.propBody?.body;
    if (body === undefined) throw new Error("the look has no prop body");
    const crate = {
      key: "",
      family: "prop",
      kind: "crate",
      variant: 1,
    } as const;
    expect(hasTint(buildGroupMesh(crate, LOOK), body)).toBe(true);
    expect(hasTint(buildPropMesh("crate", 1, LOOK), body)).toBe(false);
    // A look without a set dressing grey is handed back as it is.
    const plain: Look = { ...LOOK };
    delete plain.propBody;
    expect(propLook(plain)).toBe(plain);
    expect(hasTint(buildGroupMesh(crate, plain), body)).toBe(false);
  });

  it("builds the machines and terminals in the grey and the shell's fixtures in the plain look", () => {
    // Mutation caught: a door, portal, hatch, lift, screen, exit, poster
    // or placard dropped from the shell's fixtures (it would turn grey),
    // or a machine or terminal kept white.
    const look = LOOK;
    const shell: Fixture["kind"][] = [
      "door",
      "portal",
      "hatch",
      "lift",
      "screen",
      "exit",
      "poster",
      "placard",
    ];
    for (const kind of shell) expect(fixtureLook(kind, look), kind).toBe(look);
    for (const kind of ["machine", "terminal"] as const)
      expect(fixtureLook(kind, look).palette.machine, kind).toEqual(
        look.propBody?.body,
      );
  });

  it("never paints a hero, a curio or a fitting in the grey", () => {
    // Mutation caught: the hero family built in `propLook`. It holds a
    // kind that `propLook` would change, so the check has something to
    // catch.
    const look = LOOK;
    const grey = propLook(look);
    const same = (a: MeshData, b: MeshData) =>
      a.count === b.count &&
      Array.from(a.vertices).every((x, i) => Object.is(x, b.vertices[i]));
    let heroMoves = false;
    for (const kind of HERO_KINDS)
      for (let v = 0; v < HERO_CATALOGUE[kind].variants; v++) {
        const g = { key: "", family: "hero", kind, variant: v } as const;
        const plain = buildHeroMesh(kind, v, look);
        expect(same(buildGroupMesh(g, look), plain), kind).toBe(true);
        heroMoves ||= !same(buildHeroMesh(kind, v, grey), plain);
      }
    expect(heroMoves).toBe(true);
    // No curio or fitting draws with the body or panel colours today, so
    // for them this pins the family's look, should one ever do.
    for (const kind of CURIO_KINDS)
      for (let v = 0; v < CURIO_CATALOGUE[kind].sizes.length; v++) {
        const g = { key: "", family: "curio", kind, variant: v } as const;
        const plain = buildCurioMesh(kind, v, look);
        expect(same(buildGroupMesh(g, look), plain), kind).toBe(true);
      }
    for (const kind of INTERIOR_KINDS)
      for (let v = 0; v < INTERIOR_CATALOGUE[kind].variants; v++) {
        const g = { key: "", family: "interior", kind, variant: v } as const;
        const plain = buildInteriorMesh(kind, v, look);
        expect(same(buildGroupMesh(g, look), plain), kind).toBe(true);
      }
  }, 60_000);
});
