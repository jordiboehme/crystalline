/**
 * The station's seams on a room built elsewhere (M3 C28), and `roomFor`,
 * the one place that turns a loaded address into a room: fixtures the
 * session's station adds to a room after its generator is done, so the
 * generator's own output, and the goldens that pin it, stay as they were.
 *
 * `withExit` puts the way up on an engram room's entrance edge: an `exit`
 * that leads to the room's deck. The entrance edge is kept clear by the
 * dressing (it is never a free edge, and its lane runs from the entrance
 * wall to the hall's middle), so nothing is dressed again round it.
 *
 * `StationRoomInput` is the success shape `data/station.ts`'s `loadStation`
 * builds (kept here rather than there, so `world/` never imports `data/`:
 * M3 A23): one variant per address kind, carrying just enough of the
 * fetched data to build the room. `roomFor` turns one into the room the
 * player stands in: the airlock and a bridge and a deck's builder for their
 * own kinds, `generateRoom` plus `withExit` for an engram. A bridge or an
 * engram room also carries the `PlaceInput` it was generated from, for the
 * session's placard and CRT reader; the airlock and a deck carry none.
 *
 * Not the generator side: no generator imports this module.
 */

import { seedFor } from "../core/seed";
import { airlockRoom, type AirlockInput } from "./airlock";
import { withArrivalBox } from "./arrival";
import { withBridge, type BridgeInput } from "./bridge";
import { deckRoomSection, generateDeck, type DeckInput } from "./deck";
import { generateRoom } from "./generate";
import { deckLabel } from "./lifts";
import { edgeKey } from "./sites";
import type { PlaceInput, RoomSpec, StationAddress, WallSlot } from "./types";

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

/**
 * What `data/station.ts`'s `loadStation` built from one address, once every
 * fetch it needed has settled: just enough to build the room `roomFor`
 * hands the session (M3 A23). A deck's `section` is the one the address
 * asked for, `null` when it was unresolved (an exit's target, C28); an
 * engram's `folder` is the deck it belongs to, worked out from its detail
 * (`folderOfPath`, or `folderOfPermalink` of its permalink in a virtual
 * domain, where no file path exists).
 */
export type StationRoomInput =
  | { kind: "airlock"; input: AirlockInput }
  | { kind: "bridge"; place: PlaceInput; bridge: BridgeInput }
  | { kind: "deck"; input: DeckInput; section: number | null }
  | { kind: "engram"; place: PlaceInput; folder: string };

/**
 * The room `roomFor` builds for `loaded`: the room itself, the place it
 * was generated from (null for the airlock and a deck, which are not
 * engrams), the resolved `StationAddress` the player is now standing at (a
 * deck's `section` always a number, never `null`), and, when the room was
 * entered through the console room's exit (`landing === "box"`), the
 * arrival box's spawn point and its index among the room's heroes.
 */
export interface StationRoom {
  room: RoomSpec;
  place: PlaceInput | null;
  address: StationAddress;
  spawn: { x: number; z: number; yaw: number } | null;
  box: number | null;
}

/**
 * Turns a loaded address (`StationRoomInput`) into the room the player
 * stands in (M3 C1, C20, C21, C28):
 *
 * - an **airlock** is `airlockRoom`, its `here` (the domain the lift marks
 *   as the one the player came from) read off `arrival` - the domain of
 *   whatever `StationAddress` the player walked up from, or null when there
 *   is none or it too is the airlock - since `loadStation` has no arrival
 *   to read it from itself;
 * - a **bridge** is `generateRoom(place)` fitted with `withBridge`, then,
 *   only with `landing === "box"` (the console room's exit, C21), stood on
 *   round the arrival box through `withArrivalBox`, which composes after
 *   the fittings so the box's candidates see the lift and the screen and
 *   never stand in the screen's lane;
 * - a **deck** is `generateDeck` at the section `deckRoomSection` resolves:
 *   the section holding the engram `arrival.from` names, when it names one
 *   in this same domain, else the section `loaded.section` asked for (M3
 *   C1's `null` reading as the first);
 * - an **engram** is `generateRoom(place)` with one exit added
 *   (`withExit`) to its deck, `{ kind: "deck", domain, folder,
 *   section: null }` labelled `deckLabel(domain, folder)` - unresolved, so
 *   a walk back up always lands in the section the engram itself sits in.
 *
 * The room the player did not arrive at through the console room's exit
 * keeps `spawn` and `box` null: the session places the player at the
 * room's own `entrance` instead, as it does for any other visit.
 */
export function roomFor(
  loaded: StationRoomInput,
  arrival: { from: StationAddress } | null,
  landing: "box" | null,
): StationRoom {
  switch (loaded.kind) {
    case "airlock": {
      const here =
        arrival !== null && arrival.from.kind !== "airlock"
          ? arrival.from.domain
          : null;
      const room = airlockRoom({ domains: loaded.input.domains, here });
      return {
        room,
        place: null,
        address: { kind: "airlock" },
        spawn: null,
        box: null,
      };
    }
    case "bridge": {
      const fitted = withBridge(
        loaded.place,
        generateRoom(loaded.place),
        loaded.bridge,
      );
      const arrived =
        landing === "box" ? withArrivalBox(loaded.place, fitted) : null;
      return {
        room: arrived?.room ?? fitted,
        place: loaded.place,
        address: { kind: "bridge", domain: loaded.bridge.domain },
        spawn: arrived?.spawn ?? null,
        box: arrived?.box ?? null,
      };
    }
    case "deck": {
      const from =
        arrival !== null &&
        arrival.from.kind === "engram" &&
        arrival.from.domain === loaded.input.domain
          ? arrival.from.permalink
          : null;
      const section = deckRoomSection(loaded.input, loaded.section, from);
      return {
        room: generateDeck(loaded.input, section),
        place: null,
        address: {
          kind: "deck",
          domain: loaded.input.domain,
          folder: loaded.input.folder,
          section,
        },
        spawn: null,
        box: null,
      };
    }
    case "engram": {
      const to: StationAddress = {
        kind: "deck",
        domain: loaded.place.domain,
        folder: loaded.folder,
        section: null,
      };
      const room = withExit(
        generateRoom(loaded.place),
        to,
        deckLabel(loaded.place.domain, loaded.folder),
      );
      return {
        room,
        place: loaded.place,
        address: {
          kind: "engram",
          domain: loaded.place.domain,
          permalink: loaded.place.permalink,
        },
        spawn: null,
        box: null,
      };
    }
  }
}
