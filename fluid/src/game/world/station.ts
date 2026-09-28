/**
 * The station's seams on a room built elsewhere (M3 C28): fixtures the
 * session's station adds to a room after its generator is done, so the
 * generator's own output, and the goldens that pin it, stay as they were.
 *
 * `withExit` puts the way up on an engram room's entrance edge: an `exit`
 * that leads to the room's deck. The entrance edge is kept clear by the
 * dressing (it is never a free edge, and its lane runs from the entrance
 * wall to the hall's middle), so nothing is dressed again round it.
 *
 * Not the generator side: no generator imports this module.
 */

import { seedFor } from "../core/seed";
import { edgeKey } from "./sites";
import type { RoomSpec, StationAddress, WallSlot } from "./types";

/**
 * The room with an `exit` fixture on its entrance edge (`room.entrance`'s
 * `s` edge), leading `to` and labelled `label`, seeded
 * `seedFor(room.seed, "exit")`. The exit is appended after every other
 * fixture, so every fixture keeps its index and the text and mover keys
 * that name it (`door:<i>` and so on). A room that already has a fixture
 * on that edge (an exit put there before, say) comes back unchanged.
 */
export function withExit(
  room: RoomSpec,
  to: StationAddress,
  label: string,
): RoomSpec {
  const slot: WallSlot = { ...room.entrance, side: "s" };
  const key = edgeKey(slot);
  if (room.fixtures.some((f) => edgeKey(f.slot) === key)) return room;
  return {
    ...room,
    fixtures: [
      ...room.fixtures,
      { kind: "exit", slot, label, to, seed: seedFor(room.seed, "exit") },
    ],
  };
}
