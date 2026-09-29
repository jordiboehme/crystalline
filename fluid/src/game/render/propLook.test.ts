/**
 * Look 2's set dressing: light grey bodies (`propBody`, `propLook`) on the
 * props, the machines, the terminals and the furniture, never on the
 * shell's fixtures, the heroes, the curios or the fittings, and looks 1
 * and 3 drawn exactly as before.
 */

import { describe, expect, it } from "vitest";

import { CANNED_HUB, CANNED_WORKSHOP } from "../world/canned";
import { CURIO_CATALOGUE, CURIO_KINDS } from "../world/curios";
import { INTERIOR_CATALOGUE } from "../world/consoleRoom";
import { generateRoom } from "../world/generate";
import { HERO_CATALOGUE, HERO_KINDS } from "../world/heroes";
import { PROP_CATALOGUE, PROP_KINDS } from "../world/props";
import type { Fixture, InteriorKind } from "../world/types";
import {
  FLOATS_PER_VERTEX,
  buildRoomMesh,
  fixtureLook,
  type MeshData,
} from "./geometry";
import { LOOKS, propLook, type Look, type Rgb } from "./looks";
import { buildCurioMesh } from "./models/curios";
import { buildHeroMesh } from "./models/heroes";
import { buildInteriorMesh } from "./models/interior";
import { buildPropMesh } from "./models/props";
import { buildGroupMesh } from "./renderer";

const INTERIOR_KINDS = Object.keys(INTERIOR_CATALOGUE) as InteriorKind[];

/** Every vertex float's bits, as an FNV-1a hash, tints and flags included. */
function fullHash(meshes: readonly MeshData[]): string {
  let h = 0x811c9dc5;
  for (const m of meshes) {
    const bits = new Uint32Array(
      m.vertices.buffer,
      m.vertices.byteOffset,
      m.count * FLOATS_PER_VERTEX,
    );
    for (const w of bits)
      for (let b = 0; b < 4; b++) {
        h ^= (w >>> (8 * b)) & 0xff;
        h = Math.imul(h, 0x01000193) >>> 0;
      }
  }
  return h.toString(16).padStart(8, "0");
}

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

/** Every prop kind and variant's mesh in `look`, in catalogue order. */
function allProps(look: Look): MeshData[] {
  return PROP_KINDS.flatMap((kind) =>
    Array.from({ length: PROP_CATALOGUE[kind].variants }, (_, v) =>
      buildGroupMesh({ key: "", family: "prop", kind, variant: v }, look),
    ),
  );
}

/** The static meshes of two generated rooms in `look`. */
function rooms(look: Look): MeshData[] {
  return [CANNED_HUB, CANNED_WORKSHOP].map(
    (input) => buildRoomMesh(generateRoom(input), look).static,
  );
}

describe("look 2's light grey set dressing", () => {
  it("keeps the grey in look 2's settings, light but darker than its walls", () => {
    // Mutation caught: the grey taken off the look, a dark grey (the
    // prototype's), or one too light to stand off the walls in bright
    // light (the first tuning's #bdbdb8).
    const body = LOOKS.aperture.propBody?.body;
    if (body === undefined) throw new Error("look 2 has no prop body");
    const lum = (c: Rgb) => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    expect(lum(body)).toBeGreaterThan(0.6);
    // Dark enough to stand off the walls in the bright hangar too.
    expect(lum(body)).toBeLessThan(0.72);
    expect(lum(body)).toBeLessThan(lum(LOOKS.aperture.palette.panel) - 0.15);
  });

  it("paints the props' bodies grey in look 2 and in no other look", () => {
    // Mutation caught: `propLook` dropped from the prop family (the props
    // back in the look's white), or a grey given to look 1 or 3.
    const body = LOOKS.aperture.propBody?.body;
    if (body === undefined) throw new Error("look 2 has no prop body");
    const crate = {
      key: "",
      family: "prop",
      kind: "crate",
      variant: 1,
    } as const;
    expect(hasTint(buildGroupMesh(crate, LOOKS.aperture), body)).toBe(true);
    expect(hasTint(buildPropMesh("crate", 1, LOOKS.aperture), body)).toBe(
      false,
    );
    for (const look of [LOOKS.day, LOOKS.freescape]) {
      expect(propLook(look), look.id).toBe(look);
      expect(hasTint(buildGroupMesh(crate, look), body), look.id).toBe(false);
    }
  });

  it("builds the machines and terminals in the grey and the shell's fixtures in the plain look", () => {
    // Mutation caught: a door, portal, hatch, lift, screen, exit, poster
    // or placard dropped from the shell's fixtures (it would turn grey),
    // or a machine or terminal kept white.
    const look = LOOKS.aperture;
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
    const look = LOOKS.aperture;
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

describe("looks 1 and 3", () => {
  it("carry none of look 2's set dressing settings", () => {
    // Mutation caught: a grey body or the props' own accents given to
    // look 1 or 3.
    for (const look of [LOOKS.day, LOOKS.freescape]) {
      expect(look.propBody, look.id).toBeUndefined();
      expect(look.propAccents, look.id).toBeUndefined();
    }
  });

  it("draw every prop and two rooms' static meshes to the last bit as before look 2's set dressing", () => {
    // Mutation caught: any change to a prop or a room in look 1 or 3 (a
    // lid, band or trim drawn outside look 2, a grey or an own accent
    // leaking in, a moved rib). The hashes were taken before the change,
    // and the props' once more when the trolley's handle got real upright
    // posts in every look (the only kind that moved: its two variants).
    expect({
      dayProps: fullHash(allProps(LOOKS.day)),
      freescapeProps: fullHash(allProps(LOOKS.freescape)),
      dayRooms: fullHash(rooms(LOOKS.day)),
      freescapeRooms: fullHash(rooms(LOOKS.freescape)),
    }).toEqual({
      dayProps: "348ac20c",
      freescapeProps: "c1f5d544",
      dayRooms: "a031ce9c",
      freescapeRooms: "4941a294",
    });
  }, 60_000);
});
