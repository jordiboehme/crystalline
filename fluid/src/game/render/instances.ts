/**
 * The set dressing as instance data: one group per prop kind and variant,
 * each holding one short record per prop for the GPU.
 *
 * Every prop model is built once at the origin (`buildPropMesh` in
 * `models/props/index.ts`), and the scene vertex shader turns each instance
 * by its quarter turns and moves it to its anchor. This module is the pure
 * half of that: it reads `room.props` and writes the per-instance floats,
 * so what the renderer uploads is tested without a GL context. The
 * renderer uploads one instance buffer per group and draws the group's
 * mesh once with `drawArraysInstanced`.
 */

import type { PropKind, RoomSpec } from "../world/types";
import { CELL } from "../world/units";

/**
 * Floats per instance: the anchor in world metres (x, y, z), the quarter
 * turns and one reserved slot. The first three feed the shader's
 * `aInstanceOffset` (location 6), the last two its `aInstanceTurn`
 * (location 7). The slot is always 0 for now; it is kept for blinking
 * lamps and accent colours later, so the layout does not change again.
 */
export const INSTANCE_FLOATS = 5;

/**
 * The key a prop's mesh is cached and grouped under: `kind:variant`. The
 * look is not part of it; the renderer clears its mesh cache when the look
 * changes.
 */
export function propKey(kind: PropKind, variant: number): string {
  return `${kind}:${String(variant)}`;
}

/**
 * All instances of one kind and variant: `count` records of
 * `INSTANCE_FLOATS` floats in `data`, in the order the props appear in
 * `room.props`.
 */
export interface PropGroup {
  key: string;
  kind: PropKind;
  variant: number;
  count: number;
  data: Float32Array;
}

/**
 * The room's props as instance groups, one per distinct kind and variant,
 * sorted by key so the draw order is the same every time. Each instance is
 * `x * CELL`, the anchor height (the ceiling for a ceiling prop, else the
 * floor), `y * CELL`, the turn and a zero slot. A room without props gives
 * no groups. A pure function: the same room gives equal arrays.
 */
export function propInstances(room: RoomSpec): PropGroup[] {
  const byKey = new Map<
    string,
    { kind: PropKind; variant: number; floats: number[] }
  >();
  for (const p of room.props) {
    const key = propKey(p.kind, p.variant);
    let entry = byKey.get(key);
    if (entry === undefined) {
      entry = { kind: p.kind, variant: p.variant, floats: [] };
      byKey.set(key, entry);
    }
    entry.floats.push(
      p.x * CELL,
      p.anchor === "ceiling" ? room.ceiling : 0,
      p.y * CELL,
      p.turn,
      0,
    );
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
