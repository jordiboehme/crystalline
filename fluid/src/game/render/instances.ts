/**
 * The set dressing, the heroes, the curios and the console room's
 * fittings as instance data: one group per prop kind and variant, one per
 * hero kind and variant, one per curio kind and variant and one per
 * fitting kind and variant, each holding one short record per instance
 * for the GPU.
 *
 * Every prop model is built once at the origin (`buildPropMesh` in
 * `models/props/index.ts`), and so is every hero model (`buildHeroMesh` in
 * `models/heroes/index.ts`) and every curio model (`buildCurioMesh` in
 * `models/curios/index.ts`); the scene vertex shader turns each instance
 * by its quarter turns and moves it to its anchor. This module is the pure
 * half of that: it reads `room.props`, `room.heroes`, `room.curios` and
 * `room.interior` and
 * writes the per-instance floats, so what the renderer uploads is tested
 * without a GL context. The renderer uploads one instance buffer per group
 * and draws the group's mesh once with `drawArraysInstanced`.
 *
 * Heroes are instanced like props, in their own key space (`heroKey`,
 * `hero:<kind>:<variant>`), so a hero's mesh never shares a cache entry
 * with a prop's. Their slot is their kind's blink bank (`HERO_BANK`,
 * `bankSlot`, H11), which the fragment shader reads for their blinking
 * lights. A prop's slot is its kind's bank (`PROP_BANK`, 2.6d C15), 0 for
 * every steady kind.
 *
 * Curios are the third family (C1), in a key space of their own
 * (`curioKey`, `curio:<kind>:<variant>`). An instance's height is the
 * surface the curio stands on (`Curio.h`), not the floor, and its slot is
 * its kind's blink bank (`CURIO_BANK`, C16).
 *
 * The hand-built rooms' fittings (the console room's, 2.6e C2; the
 * airlock's, M3 C24) are the fourth family, in a key space of their own
 * (`interiorKey`, `interior:<kind>:<variant>`), built once at the origin
 * by `buildInteriorMesh` (`models/interior/index.ts`). An instance stands
 * at floor level (a fitting hung from the ceiling, the airlock's iris
 * light, is built at its own height in its mesh), and its slot is its
 * kind's blink bank (`INTERIOR_BANK`), which the console's small lights,
 * the roundels' glow and the airlock's beacons pulse with. Only a room
 * with `interior` has any.
 */

import type {
  CurioKind,
  HeroKind,
  InteriorKind,
  Prop,
  PropKind,
  RoomSpec,
} from "../world/types";
import { CELL } from "../world/units";
import { bankSlot } from "./blink";
import { propAccentPick } from "./looks";
import { CURIO_BANK } from "./models/curios/common";
import { HERO_BANK } from "./models/heroes/common";
import { INTERIOR_BANK } from "./models/interior/common";
import { PROP_BANK } from "./models/props/common";

/**
 * Floats per instance: the anchor in world metres (x, y, z), the quarter
 * turns and the slot. The first three feed the shader's `aInstanceOffset`
 * (location 6), the last two its `aInstanceTurn` (location 7). The slot is
 * the instance's blink bank (`bankSlot`, H11): its kind's bank, whether
 * it is a hero, a curio or a prop (`PROP_BANK`, 0 for a steady prop).
 */
export const INSTANCE_FLOATS = 5;

/**
 * A prop instance's turn float: its quarter turn, 0 to 3 (a stray turn
 * wrapped, so it never shifts the pick), plus four times
 * one more than its accent pick (`propAccentPick` of its seed), so 4 to 23.
 * The vertex shader turns the instance by the float's low two bits and
 * takes the prop's own accent (`PROP_MARK`) from the rest; a hero, a curio
 * or a fitting writes its bare turn, 0 to 3, so it has no pick and its
 * marks take the room's accent. Every look reads the same float: a mesh
 * without the prop's own mark never uses the pick.
 */
export function propTurn(p: Pick<Prop, "turn" | "seed">): number {
  return (p.turn & 3) + 4 * (1 + propAccentPick(p.seed));
}

/**
 * The key a prop's mesh is cached and grouped under: `kind:variant`. The
 * look is not part of it; the renderer clears its mesh cache when the look
 * changes.
 */
export function propKey(kind: PropKind, variant: number): string {
  return `${kind}:${String(variant)}`;
}

/**
 * The key a hero's mesh is cached and grouped under: `hero:<kind>:<variant>`.
 * The prefix keeps it apart from every `propKey`, so the renderer's one
 * mesh cache holds both families without a collision.
 */
export function heroKey(kind: HeroKind, variant: number): string {
  return `hero:${kind}:${String(variant)}`;
}

/**
 * The key a curio's mesh is cached and grouped under:
 * `curio:<kind>:<variant>`. The prefix keeps it apart from every `propKey`
 * and every `heroKey`, so the renderer's one mesh cache holds all three
 * families without a collision.
 */
export function curioKey(kind: CurioKind, variant: number): string {
  return `curio:${kind}:${String(variant)}`;
}

/**
 * The key a console room fitting's mesh is cached and grouped under:
 * `interior:<kind>:<variant>`. The prefix keeps it apart from every
 * `propKey`, `heroKey` and `curioKey`, so the renderer's one mesh cache
 * holds all four families without a collision.
 */
export function interiorKey(kind: InteriorKind, variant: number): string {
  return `interior:${kind}:${String(variant)}`;
}

/**
 * All instances of one prop kind and variant: `count` records of
 * `INSTANCE_FLOATS` floats in `data`, in the order the props appear in
 * `room.props`.
 */
export interface PropGroup {
  key: string;
  family: "prop";
  kind: PropKind;
  variant: number;
  count: number;
  data: Float32Array;
}

/**
 * All instances of one hero kind and variant: `count` records of
 * `INSTANCE_FLOATS` floats in `data`, in the order the heroes appear in
 * `room.heroes`.
 */
export interface HeroGroup {
  key: string;
  family: "hero";
  kind: HeroKind;
  variant: number;
  count: number;
  data: Float32Array;
}

/**
 * All instances of one curio kind and variant: `count` records of
 * `INSTANCE_FLOATS` floats in `data`, in the order the curios appear in
 * `room.curios`.
 */
export interface CurioGroup {
  key: string;
  family: "curio";
  kind: CurioKind;
  variant: number;
  count: number;
  data: Float32Array;
}

/**
 * All instances of one console room fitting kind and variant: `count`
 * records of `INSTANCE_FLOATS` floats in `data`, in the order the pieces
 * appear in `room.interior`.
 */
export interface InteriorGroup {
  key: string;
  family: "interior";
  kind: InteriorKind;
  variant: number;
  count: number;
  data: Float32Array;
}

/**
 * One instance group of any of the four families; `family` tells them apart.
 */
export type InstanceGroup = PropGroup | HeroGroup | CurioGroup | InteriorGroup;

/**
 * Groups records by key and sorts the groups by key, so the draw order is
 * the same every time: `records` gives each item's key, kind, variant and
 * floats.
 */
function grouped<K>(
  records: readonly {
    key: string;
    kind: K;
    variant: number;
    floats: number[];
  }[],
): {
  key: string;
  kind: K;
  variant: number;
  count: number;
  data: Float32Array;
}[] {
  const byKey = new Map<
    string,
    { kind: K; variant: number; floats: number[] }
  >();
  for (const r of records) {
    let entry = byKey.get(r.key);
    if (entry === undefined) {
      entry = { kind: r.kind, variant: r.variant, floats: [] };
      byKey.set(r.key, entry);
    }
    entry.floats.push(...r.floats);
  }
  return [...byKey.keys()].sort().flatMap((key) => {
    const entry = byKey.get(key);
    if (entry === undefined) return [];
    return [
      {
        key,
        kind: entry.kind,
        variant: entry.variant,
        count: entry.floats.length / INSTANCE_FLOATS,
        data: new Float32Array(entry.floats),
      },
    ];
  });
}

/**
 * The room's props as instance groups, one per distinct kind and variant,
 * sorted by key so the draw order is the same every time. Each instance is
 * `x * CELL`, the anchor height (the ceiling for a ceiling prop, else the
 * floor), `y * CELL`, the turn with its accent pick (`propTurn`) and its
 * kind's blink bank slot
 * (`bankSlot(PROP_BANK[kind])`, 2.6d C15: 0 for every steady kind). A room
 * without props gives no groups. A pure function: the same room gives
 * equal arrays.
 */
export function propInstances(room: RoomSpec): PropGroup[] {
  return grouped(
    room.props.map((p) => ({
      key: propKey(p.kind, p.variant),
      kind: p.kind,
      variant: p.variant,
      floats: [
        p.x * CELL,
        p.anchor === "ceiling" ? room.ceiling : 0,
        p.y * CELL,
        propTurn(p),
        bankSlot(PROP_BANK[p.kind]),
      ],
    })),
  ).map((g) => ({ ...g, family: "prop" as const }));
}

/**
 * The room's heroes as instance groups, one per distinct kind and variant,
 * sorted by key. Each instance is `x * CELL`, 0 (the instance stands on the
 * floor; a hovering hero's mesh is built at its lift, `heroLift`, C4, so the
 * three floaters hover without a height of their own here), `y * CELL`, the
 * turn and its kind's blink bank slot (`bankSlot(HERO_BANK[kind])`). A room
 * without heroes gives no groups. A pure function: the same room gives
 * equal arrays.
 */
export function heroInstances(room: RoomSpec): HeroGroup[] {
  return grouped(
    room.heroes.map((h) => ({
      key: heroKey(h.kind, h.variant),
      kind: h.kind,
      variant: h.variant,
      floats: [h.x * CELL, 0, h.y * CELL, h.turn, bankSlot(HERO_BANK[h.kind])],
    })),
  ).map((g) => ({ ...g, family: "hero" as const }));
}

/**
 * The room's curios as instance groups, one per distinct kind and variant,
 * sorted by key. Each instance is `x * CELL`, the height of the surface it
 * stands on (`c.h`), `y * CELL`, the turn and its kind's blink bank slot
 * (`bankSlot(CURIO_BANK[kind])`). A room without curios gives no groups. A
 * pure function: the same room gives equal arrays.
 */
export function curioInstances(room: RoomSpec): CurioGroup[] {
  return grouped(
    room.curios.map((c) => ({
      key: curioKey(c.kind, c.variant),
      kind: c.kind,
      variant: c.variant,
      floats: [
        c.x * CELL,
        c.h,
        c.y * CELL,
        c.turn,
        bankSlot(CURIO_BANK[c.kind]),
      ],
    })),
  ).map((g) => ({ ...g, family: "curio" as const }));
}

/**
 * The hand-built rooms' fittings as instance groups, one per distinct
 * kind and variant, sorted by key. Each instance is `x * CELL`, 0 (every
 * fitting's mesh starts at floor level), `y * CELL`, the turn and its kind's blink
 * bank slot (`bankSlot(INTERIOR_BANK[kind])`). A room without `interior`
 * (every generated room) gives no groups. A pure function: the same room
 * gives equal arrays.
 */
export function interiorInstances(room: RoomSpec): InteriorGroup[] {
  return grouped(
    (room.interior ?? []).map((p) => ({
      key: interiorKey(p.kind, p.variant),
      kind: p.kind,
      variant: p.variant,
      floats: [
        p.x * CELL,
        0,
        p.y * CELL,
        p.turn,
        bankSlot(INTERIOR_BANK[p.kind]),
      ],
    })),
  ).map((g) => ({ ...g, family: "interior" as const }));
}

/**
 * Every instance group the renderer draws: the props' groups, then the
 * heroes', then the curios', then the console room fittings', each family
 * sorted by key.
 */
export function instanceGroups(room: RoomSpec): InstanceGroup[] {
  return [
    ...propInstances(room),
    ...heroInstances(room),
    ...curioInstances(room),
    ...interiorInstances(room),
  ];
}
