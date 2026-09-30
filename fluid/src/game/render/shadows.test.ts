import { describe, expect, it } from "vitest";

import { CANNED_WORKSHOP } from "../world/canned";
import { propFootprint } from "../world/footprints";
import { generateRoom } from "../world/generate";
import { wallAnchor } from "../world/sites";
import type { Prop, RoomSpec } from "../world/types";
import { CELL } from "../world/units";
import { LOOKS } from "./looks";
import {
  MARGIN,
  SHADOW_TEXELS,
  casterDarkness,
  casterSoftness,
  contactShadows,
  shadowCasters,
} from "./shadows";

const workshop = generateRoom(CANNED_WORKSHOP);

/** The workshop emptied, then given only `props`. */
function roomWith(props: Prop[]): RoomSpec {
  return {
    ...workshop,
    fixtures: [],
    decor: [],
    heroes: [],
    interior: [],
    scaffold: [],
    props,
  };
}

/** A floor prop at cell point (4, 4). */
function floorProp(kind: Prop["kind"], variant: number, turn = 0): Prop {
  return { kind, variant, anchor: "floor", x: 4, y: 4, turn, seed: 3 };
}

/** The shadow's darkness at world point `(x, z)`, 0 to 1. */
function sample(room: RoomSpec, x: number, z: number): number {
  const s = contactShadows(room);
  const tx = Math.floor(x * SHADOW_TEXELS);
  const tz = Math.floor(z * SHADOW_TEXELS);
  return (s.texels[tz * s.width + tx] ?? NaN) / 255;
}

const CX = 4 * CELL;
const CZ = 4 * CELL;

describe("contact shadows", () => {
  it("sizes each prop's patch to its footprint and turns it with the prop", () => {
    // Mutation caught: a patch of one size for every prop (the
    // prototype's uniform blob), width and depth not swapped at a quarter
    // turn, or a patch centred off the prop.
    for (const turn of [0, 1, 2, 3]) {
      const p = floorProp("bench", 0, turn);
      const box = propFootprint(p);
      if (box === null) throw new Error("no footprint");
      const [c] = shadowCasters(roomWith([p]));
      expect(c, `turn ${String(turn)}`).toEqual({
        x: (box.x0 + box.x1) / 2,
        z: (box.z0 + box.z1) / 2,
        hx: (box.x1 - box.x0) / 2,
        hz: (box.z1 - box.z0) / 2,
        round: false,
      });
    }
    // The bench is 1.4 m wide and 0.5 m deep: 0.6 m out along its width
    // lies under it, 0.6 m out along its depth lies clear of it.
    const along = roomWith([floorProp("bench", 0, 0)]);
    const across = roomWith([floorProp("bench", 0, 1)]);
    expect(sample(along, CX + 0.6, CZ)).toBeGreaterThan(0.5);
    expect(sample(along, CX, CZ + 0.6)).toBeLessThan(0.1);
    expect(sample(across, CX, CZ + 0.6)).toBeGreaterThan(0.5);
    expect(sample(across, CX + 0.6, CZ)).toBeLessThan(0.1);
    // Nearly full darkness half a margin past the footprint's edge, so
    // the shadow shows round the prop's foot, then nothing past the soft
    // edge.
    const [c] = shadowCasters(along);
    if (c === undefined) throw new Error("no caster");
    expect(MARGIN).toBeGreaterThan(0);
    expect(sample(along, CX + c.hx + MARGIN / 2, CZ)).toBeGreaterThan(
      0.85 * casterDarkness(c),
    );
    const clear = c.hx + MARGIN + casterSoftness(c) + 1 / SHADOW_TEXELS;
    expect(sample(along, CX + clear, CZ)).toBe(0);
  });

  it("gives a round prop a soft disc, darker at its edge's middle than at its footprint's corner", () => {
    // Mutation caught: a barrel given a rectangle, or every prop a disc.
    const barrel = roomWith([floorProp("barrel", 0)]);
    const crate = roomWith([floorProp("crate", 0)]);
    expect(shadowCasters(barrel)[0]?.round).toBe(true);
    expect(shadowCasters(crate)[0]?.round).toBe(false);
    // Both are square footprints (0.65 m and 0.8 m); sample each at 90
    // percent of its half-width, straight out and at the corner.
    const at = (room: RoomSpec, h: number) => ({
      edge: sample(room, CX + 0.9 * h, CZ),
      corner: sample(room, CX + 0.9 * h, CZ + 0.9 * h),
    });
    const b = at(barrel, 0.325);
    const k = at(crate, 0.4);
    expect(b.corner).toBeLessThan(b.edge * 0.6);
    expect(k.corner).toBeGreaterThan(k.edge * 0.8);
  });

  it("keeps small and medium props' patches full, lightens a large one's by its size, and widens its soft edge", () => {
    // Mutation caught: one darkness for every size (big cabinets and
    // machines back on a painted pad), a darkness that grows with the
    // footprint, one keyed by kind rather than size (two equal footprints
    // would differ), a floor other than 0.55 (0.5), another slope, or
    // softness without bound.
    const at = (hx: number, hz: number, round = false) => ({
      x: 0,
      z: 0,
      hx,
      hz,
      round,
    });
    const barrel = at(0.325, 0.325, true);
    const crate = at(0.4, 0.4);
    const machine = at(0.9, 0.45);
    const console = at(1.5, 0.5);
    const hero = at(2.5, 1.3);
    expect(casterDarkness(barrel)).toBe(1);
    expect(casterDarkness(crate)).toBe(1);
    expect(casterDarkness(machine)).toBeLessThan(0.9);
    expect(casterDarkness(console)).toBeLessThan(casterDarkness(machine));
    expect(casterDarkness(hero)).toBeLessThan(casterDarkness(console));
    // The floor and the slope, exactly: a large hero sits on the floor,
    // a 3 m by 1 m console at 1 - 0.35 * (1.732 - 0.8).
    expect(casterDarkness(hero)).toBeCloseTo(0.55, 5);
    expect(casterDarkness(console)).toBeCloseTo(
      1 - 0.35 * (2 * Math.sqrt(0.75) - 0.8),
      5,
    );
    // Size alone: a round and a square patch of one footprint match.
    expect(casterDarkness(at(0.9, 0.45, true))).toBe(casterDarkness(machine));
    expect(casterSoftness(hero)).toBeGreaterThan(casterSoftness(barrel));
    expect(casterSoftness(at(10, 10))).toBeLessThanOrEqual(0.5);
  });

  it("gives a hero standing on the floor a patch, a flush hero none, and the console room's fittings theirs", () => {
    // Mutation caught: flush heroes (on their wall) given a patch, or the
    // fittings dropped from the casters.
    const room: RoomSpec = {
      ...roomWith([]),
      heroes: [
        { kind: "turret", variant: 0, x: 3, y: 3, turn: 0, seed: 1 },
        { kind: "eye-panel", variant: 0, x: 5, y: 2, turn: 0, seed: 2 },
      ],
      interior: [{ kind: "console", variant: 0, x: 4, y: 4, turn: 0, seed: 3 }],
    };
    const cs = shadowCasters(room);
    // The turret (0.9 m, round) and the console (2.4 m); no eye panel.
    expect(cs.length).toBe(2);
    expect(cs.map((c) => c.round)).toEqual([true, false]);
    expect(cs[0]?.hx).toBeCloseTo(0.45, 5);
    expect(cs[1]?.hx).toBeCloseTo(1.2, 5);
  });

  it("casts a wall-standing locker bank's patch in front of its wall, along it", () => {
    // Mutation caught: the patch behind the wall (outside the cell the
    // wall bounds), or not turned along the wall.
    for (const side of ["n", "s", "w", "e"] as const) {
      const at = wallAnchor({ x: 3, y: 4, side });
      const p: Prop = {
        kind: "locker-bank",
        variant: 0,
        anchor: "wall",
        ...at,
        seed: 1,
      };
      const [c] = shadowCasters(roomWith([p]));
      if (c === undefined) throw new Error("no caster");
      const alongX = side === "n" || side === "s";
      expect(alongX ? c.hx : c.hz, side).toBeCloseTo(0.85, 5);
      expect(alongX ? c.hz : c.hx, side).toBeCloseTo(0.13, 5);
      // Its centre stands in the cell the wall bounds, 0.13 m off the wall.
      expect(c.x, side).toBeGreaterThan(3 * CELL);
      expect(c.x, side).toBeLessThan(4 * CELL);
      expect(c.z, side).toBeGreaterThan(4 * CELL);
      expect(c.z, side).toBeLessThan(5 * CELL);
      expect(Math.hypot(c.x - at.x * CELL, c.z - at.y * CELL)).toBeCloseTo(
        0.13,
        5,
      );
    }
  });

  it("shows on look 2's dark floor without blacking it out, and appears only in look 2", () => {
    // Mutation caught: a contact shadow in look 1 or 3, one back at the
    // first strength (0.7, too faint on the dark floor), or one that
    // blacks the floor out.
    expect(LOOKS.day.contactShadow).toBeUndefined();
    expect(LOOKS.freescape.contactShadow).toBeUndefined();
    const k = LOOKS.aperture.contactShadow ?? 0;
    expect(k).toBeGreaterThan(0.75);
    expect(k).toBeLessThanOrEqual(0.9);
  });
});
