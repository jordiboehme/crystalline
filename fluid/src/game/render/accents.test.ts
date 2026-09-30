/**
 * Which families carry the room's accent (2.7 C9): six prop kinds' small
 * parts (the stool's seat, the bench's seat, the trolley's handle, the tool
 * cart's drawer fronts, the barrel's band and the locker bank's trim strip)
 * and nowhere else in the set dressing. A hero, a curio, a fitting, a way
 * (a door, a portal, a hatch), a poster, the placard and a tag strip keep
 * their own colours or their meaning in every look, so none of them may
 * carry the mark. A machine carries its tag's second accent, baked in
 * (2.7 C10), never the room's; a terminal carries the room's by design.
 */

import { describe, expect, it } from "vitest";

import { CANNED_HUB } from "../world/canned";
import { CURIO_CATALOGUE, CURIO_KINDS } from "../world/curios";
import { INTERIOR_CATALOGUE } from "../world/consoleRoom";
import { MACHINE_KINDS, generateRoom } from "../world/generate";
import { HERO_CATALOGUE, HERO_KINDS } from "../world/heroes";
import { PROP_CATALOGUE, PROP_KINDS } from "../world/props";
import type { InteriorKind, RoomSpec } from "../world/types";
import { VARIANT_COUNTS } from "../world/variants";
import {
  ACCENT_MARK,
  FLOATS_PER_VERTEX,
  createBuilder,
  type MeshData,
} from "./geometry";
import { createKit } from "./kit";
import { layerPlan } from "./layers";
import { LOOKS } from "./looks";
import { buildFixture, type ModelContext } from "./models";
import { buildCurioMesh } from "./models/curios";
import { buildHeroMesh } from "./models/heroes";
import { buildInteriorMesh } from "./models/interior";
import { buildPropMesh } from "./models/props";

/** Every fitting kind (`consoleRoom.ts` names no `INTERIOR_KINDS` list of its own). */
const INTERIOR_KINDS = Object.keys(INTERIOR_CATALOGUE) as InteriorKind[];

/** The prop kinds whose small parts carry the room's accent (2.7 C9). */
const ACCENTED = [
  "stool",
  "bench",
  "trolley",
  "tool-cart",
  "barrel",
  "locker-bank",
] as const;

/** Every vertex's tint (floats 9 to 11 of `FLOATS_PER_VERTEX`: position 3, normal 3, uv 2, layer 1, tint 3, flag 1). */
function tints(m: MeshData): [number, number, number][] {
  return Array.from({ length: m.count }, (_, i) => {
    const o = i * FLOATS_PER_VERTEX;
    return [
      m.vertices[o + 9] ?? 0,
      m.vertices[o + 10] ?? 0,
      m.vertices[o + 11] ?? 0,
    ];
  });
}

/** The `ModelContext` `buildRoomMesh` builds for `room`, so a fixture built here sees the same text layout. */
function contextFor(room: RoomSpec): ModelContext {
  const plan = layerPlan(room);
  return {
    look: LOOKS.aperture,
    ceiling: room.ceiling,
    hall: room.hall,
    textLayer: (key) => plan.lookup(key),
  };
}

/** Whether any vertex of `m` carries the accent mark, whatever its `k`. */
const marked = (m: MeshData) => tints(m).some((t) => t[0] === ACCENT_MARK);

describe("the room's accent (2.7 C9)", () => {
  it("puts the room's accent on the six kinds' small parts in every variant, and nowhere else in the set dressing, in every look without the props' own accents", () => {
    // Mutation caught: an accented kind without its part, or the accent on
    // a kind that should not carry it. Look 2 gives each prop its own
    // accent instead (`propLook.test.ts`).
    expect(PROP_KINDS.length).toBeGreaterThan(0);
    for (const look of [LOOKS.day, LOOKS.freescape])
      for (const kind of PROP_KINDS)
        for (let v = 0; v < PROP_CATALOGUE[kind].variants; v++)
          expect(
            marked(buildPropMesh(kind, v, look)),
            `${look.id} ${kind} variant ${String(v)}`,
          ).toBe((ACCENTED as readonly string[]).includes(kind));
  });

  it("never puts the room's accent on a hero, a curio, a fitting, a way, a poster, the placard or a tag strip", () => {
    // Mutation caught: an accent leaking into a hero (its original's
    // colours), a curio, the console room, or a fixture whose colour
    // means something.
    expect(HERO_KINDS.length).toBeGreaterThan(0);
    for (const kind of HERO_KINDS)
      for (let v = 0; v < HERO_CATALOGUE[kind].variants; v++)
        expect(
          marked(buildHeroMesh(kind, v, LOOKS.aperture)),
          `${kind} variant ${String(v)}`,
        ).toBe(false);
    expect(CURIO_KINDS.length).toBeGreaterThan(0);
    for (const kind of CURIO_KINDS)
      for (let v = 0; v < CURIO_CATALOGUE[kind].sizes.length; v++)
        expect(
          marked(buildCurioMesh(kind, v, LOOKS.aperture)),
          `${kind} variant ${String(v)}`,
        ).toBe(false);
    expect(INTERIOR_KINDS.length).toBeGreaterThan(0);
    for (const kind of INTERIOR_KINDS)
      for (let v = 0; v < INTERIOR_CATALOGUE[kind].variants; v++)
        expect(
          marked(buildInteriorMesh(kind, v, LOOKS.aperture)),
          `${kind} variant ${String(v)}`,
        ).toBe(false);
    const room = generateRoom(CANNED_HUB);
    expect(room.fixtures.length).toBeGreaterThan(0);
    for (const [i, fx] of room.fixtures.entries()) {
      if (fx.kind === "terminal" || fx.kind === "machine") continue;
      const b = createBuilder();
      buildFixture((f) => createKit(b, f), fx, i, contextFor(room));
      expect(marked(b.build()), `fixture ${String(i)} (${fx.kind})`).toBe(
        false,
      );
    }
  }, 30_000);

  it("never puts the room's accent on a machine or its tag strip, in any variant (2.7 C9, C10)", () => {
    // Mutation caught: a machine recipe's trim drawn with `s.accent()` (the
    // room's accent instead of the tag's baked second accent), or a tag
    // strip that gains the mark.
    const room = generateRoom(CANNED_HUB);
    const i = room.fixtures.findIndex((f) => f.kind === "machine");
    const fx = room.fixtures[i];
    if (fx?.kind !== "machine") throw new Error("the hub holds no machine");
    expect(MACHINE_KINDS.length).toBe(12);
    for (const machine of MACHINE_KINDS)
      for (let v = 0; v < VARIANT_COUNTS.machine[machine]; v++) {
        const b = createBuilder();
        buildFixture(
          (f) => createKit(b, f),
          { ...fx, machine, variant: v },
          i,
          contextFor(room),
        );
        expect(marked(b.build()), `${machine} variant ${String(v)}`).toBe(
          false,
        );
      }
  });
});
