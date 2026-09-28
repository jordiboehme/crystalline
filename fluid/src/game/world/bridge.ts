/**
 * The bridge's own fittings (M3 C20, C21): a re-dress seam, like
 * `withHeroes` (`generate.ts`), that stands the deck lift and the domain's
 * wall screen in a room `generateRoom` already built.
 *
 * `withBridge(place, room, bridge)` appends two fixtures to the room's own,
 * in order:
 *
 * 1. the deck lift, on the entrance edge (`{ ...room.entrance, side: "s" }`,
 *    the same edge `withExit` puts an exit on), whose stops come from
 *    `bridgeStops` (`bridge.domain`, the routing key, so every stop's `to`
 *    addresses a domain that exists) and whose `note` marks a failed tree
 *    (`bridge.folders === null`, `?DECK LIST ERROR`) or an empty domain
 *    (no folders and no root deck, `NO DECKS`); otherwise the panel carries
 *    no note;
 * 2. the wall screen, on `bridgeScreenEdge(room)` when one is free: two
 *    lines, `bridge.display` (the domain's display name, kept apart from
 *    `bridge.domain` so a canonical name never leaks into a route or a
 *    stencil hash) and, when the listing's count is known, `engramCount` of
 *    it, the key pictogram on the name (`keys: [0]`) for a private domain.
 *    A room with no free edge gets no screen; the placard at the entrance
 *    already names the room, so the lift panel needs no fallback (M3 A13).
 *
 * With the two fixtures in place the room is re-dressed exactly as
 * `withHeroes` re-dresses round a forced hero: `dressRoom` of the room with
 * the new fixtures, then `placeCurios` with the room's own draws
 * (`curioDraws`) and neighbours (`nearFor(place)`), then `placeDecals`. The
 * lights, the finish and the heroes are the room's own and never move: the
 * bridge adds fixtures, never heroes. Handed no bridge data,
 * `withBridge` gives the room back unchanged (`bridge === null`), the
 * object itself, not a copy.
 *
 * `bridgeScreenEdge(room)` is `withBridge`'s own search, exported so a test
 * can call it on a room built by hand: the free wall edge (M3 C20) - a
 * wall edge with no fixture, no wall prop and not one a hero reserves -
 * nearest the hall's centre line, tried north first, then east, then west,
 * the smaller coordinate breaking a tie. "No wall prop" reads the room's
 * own `props` (`p.anchor === "wall"`, the edge recovered with `edgeOf`, as
 * `curios.ts` and `decals.ts` do); "not one a hero reserves" reads
 * `heroReserve(room.heroes)`, both its edges and its boxes grown against
 * the screen's own viewing lane (`SHEET_LANE_WIDTH` by `SHEET_LANE_DEPTH`,
 * `props.ts`), since the heroes are not re-drawn here and so must already
 * be clear of wherever the screen ends up. A room with no free edge on any
 * of the three walls gives null.
 *
 * The generator side (M3 global constraints): this module may import
 * `generate.ts` (for `nearFor`), never `move.ts`, `interact.ts`,
 * `station.ts`, `ui/` or `render/`.
 */

import { seedFor } from "../core/seed";
import { curioDraws, placeCurios } from "./curios";
import { placeDecals } from "./decals";
import { dressRoom } from "./dress";
import { footprint } from "./footprints";
import { nearFor } from "./generate";
import { heroReserve } from "./heroes";
import { LIFT_WORDS, bridgeStops, engramCount } from "./lifts";
import { SHEET_LANE_DEPTH, SHEET_LANE_WIDTH } from "./props";
import { dressingSites, edgeKey, edgeOf, overlaps } from "./sites";
import type {
  Fixture,
  LiftStop,
  PlaceInput,
  RoomSpec,
  Side,
  WallSlot,
} from "./types";

/**
 * One domain's bridge as `withBridge` needs it: `domain` the routing key
 * its lift's stops address decks with and `folderDeck` hashes decks by
 * (M3 C1, C6), `display` the name the screen reads instead - the domain's
 * canonical name when the listing knows one, else `domain` itself, so a
 * canonical name never turns into a route or a stencil hash - `engrams`
 * the listing's count (null when it failed, never drawn on the screen
 * then), `private` whether the name is drawn with the key pictogram,
 * `folders` the top-level folder names of its tree (null when the tree
 * failed to load, never an empty successful listing read as a failure),
 * and `rootDeck` whether the root holds engrams besides the MANIFEST (M3
 * C7, C12).
 */
export interface BridgeInput {
  domain: string;
  display: string;
  engrams: number | null;
  private: boolean;
  /** null when the tree failed. */
  folders: readonly string[] | null;
  rootDeck: boolean;
}

/** The order `bridgeScreenEdge` tries a bridge's walls (M3 C20). */
const SCREEN_SIDES: readonly Side[] = ["n", "e", "w"];

/** The screen's viewing lane, the size `dressingSites` gives a screen. */
const SCREEN_LANE = { along: SHEET_LANE_WIDTH, out: SHEET_LANE_DEPTH };

/**
 * The lift's stops and status note for a bridge (M3 C7, C12, C23, C29): a
 * failed tree lists `AIRLOCK` alone with `?DECK LIST ERROR`; a domain with
 * no folders and no root deck lists `AIRLOCK` alone with `NO DECKS`;
 * otherwise the full stop list with no note.
 */
function bridgePanel(bridge: BridgeInput): {
  stops: LiftStop[];
  note: string | null;
} {
  if (bridge.folders === null)
    return {
      stops: bridgeStops(bridge.domain, [], false),
      note: LIFT_WORDS.deckError,
    };
  if (bridge.folders.length === 0 && !bridge.rootDeck)
    return {
      stops: bridgeStops(bridge.domain, [], false),
      note: LIFT_WORDS.noDecks,
    };
  return {
    stops: bridgeStops(bridge.domain, bridge.folders, bridge.rootDeck),
    note: null,
  };
}

/**
 * The free wall edge the bridge's screen goes on (M3 C20), or null when
 * none of the three walls has one. See the module doc for the rule.
 */
export function bridgeScreenEdge(room: RoomSpec): WallSlot | null {
  const sites = dressingSites(room);
  const walled = new Set<string>();
  for (const p of room.props)
    if (p.anchor === "wall") walled.add(edgeKey(edgeOf(p)));
  const reserved = heroReserve(room.heroes);
  const edges = sites.runs.flat();
  for (const side of SCREEN_SIDES) {
    const alongX = side === "n" || side === "s";
    const centre = alongX
      ? (room.hall.x0 + room.hall.x1) / 2
      : (room.hall.y0 + room.hall.y1) / 2;
    let best: WallSlot | null = null;
    let bestDist = Infinity;
    let bestCoord = Infinity;
    for (const e of edges) {
      if (e.side !== side) continue;
      const key = edgeKey(e);
      if (!sites.free.has(key)) continue;
      if (walled.has(key)) continue;
      if (reserved.edges.has(key)) continue;
      const box = footprint(e, SCREEN_LANE);
      if (reserved.boxes.some((b) => overlaps(box, b))) continue;
      const coord = alongX ? e.x : e.y;
      const dist = Math.abs(coord + 0.5 - centre);
      if (dist < bestDist || (dist === bestDist && coord < bestCoord)) {
        bestDist = dist;
        bestCoord = coord;
        best = e;
      }
    }
    if (best !== null) return best;
  }
  return null;
}

/**
 * `room` with the bridge's deck lift and wall screen standing in it (M3
 * C20, C21), the props, curios and decals re-run round them exactly as
 * `withHeroes` re-runs them round a forced hero. Handed no bridge data the
 * room comes back exactly as it was handed in. See the module doc.
 */
export function withBridge(
  place: PlaceInput,
  room: RoomSpec,
  bridge: BridgeInput | null,
): RoomSpec {
  if (bridge === null) return room;
  const { stops, note } = bridgePanel(bridge);
  const lift: Fixture = {
    kind: "lift",
    slot: { ...room.entrance, side: "s" },
    stops,
    note,
    seed: seedFor(room.seed, "lift"),
  };
  const fixtures: Fixture[] = [...room.fixtures, lift];
  const edge = bridgeScreenEdge(room);
  if (edge !== null) {
    const lines =
      bridge.engrams === null
        ? [bridge.display]
        : [bridge.display, engramCount(bridge.engrams)];
    fixtures.push({
      kind: "screen",
      slot: edge,
      lines,
      keys: bridge.private ? [0] : [],
      seed: seedFor(room.seed, "screen"),
    });
  }
  const fitted: RoomSpec = { ...room, fixtures };
  const dressed: RoomSpec = { ...fitted, props: dressRoom(fitted) };
  const placed: RoomSpec = {
    ...dressed,
    curios: placeCurios(dressed, curioDraws(dressed), nearFor(place)),
  };
  return { ...placed, decals: placeDecals(placed) };
}
