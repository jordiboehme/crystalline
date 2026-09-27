/**
 * The police box in play: its front wall, the focus on it, the door steps,
 * the walk-in through its open doors, and which domain a walk out of the
 * console room's own inner doors leads to.
 *
 * A police box's two door leaves are movers (`boxLeafMovers` in
 * `render/models/heroes/street.ts`) that open and close together, so both
 * share one key, named after the hero's place in `room.heroes`: the door
 * state the session keeps under that key turns both leaves at once. The
 * key never collides with a fixture's (`door:<i>`, `lamp:<i>`,
 * `spark:<i>`, `lid:<i>`, `disc:<i>`), since a hero is not a fixture.
 *
 * - `boxFront` gives a police box's front wall, in the same terms
 *   `interact.ts`'s `wallPoint` gives a fixture's wall.
 * - `boxFocus` is the box version of `focusOf`: Space at the front toggles
 *   the doors, like a bulkhead or blast door. A fixture in focus always
 *   wins over a box (the session tries `focusOf` first).
 * - `stepBoxDoors` is the box version of `stepDoors`: the doors keep
 *   heading where they were sent until pressed again: they never close by
 *   themselves.
 * - `boxEntry` is the box version of `travelOf`, but of the box's own
 *   doors, not a way out of the room: a police box leads nowhere the
 *   generator ever built, so it is the session's business, not this
 *   module's, to decide what a walk through it does.
 * - `DomainRow`, `exitSeed` and `pickExitDomain` are the console room's own
 *   business: which of the domains the sidebar's listing knows about the
 *   inner doors lead out to (2.6e C13). `DomainRow` lives here rather than
 *   in the data layer so the world side never imports it: `data/source.ts`
 *   imports `DomainRow` from here instead. The domain the room was entered
 *   from is matched against the listing through `domainSpellings`
 *   (`../../domainNames`), the same table a door's own link resolves
 *   through, so an alias or a canonical name never excludes the wrong
 *   domain on a naming collision.
 *
 * The generator (`generate.ts`) never imports this module: a police box's
 * doors, and the console room's exit, are a session concern, like a
 * fixture door's own state, not something a room is built with.
 */

import { seedFor, createRng } from "../core/seed";
import { domainSpellings } from "../../domainNames";
import { REACH, FACING, type DoorState, type WallPoint } from "./interact";
import { HERO_FRONT, heroTurn, turnedPoint } from "./footprints";
import type { Player } from "./move";
import type { Hero, RoomSpec } from "./types";

/**
 * The mover key of the police box at `index` in `room.heroes`:
 * `box:<index>`, shared by both of its door leaves.
 */
export function boxKey(index: number): string {
  return `box:${String(index)}`;
}

/**
 * How far a police box's doors move towards open or shut in one 35 Hz
 * tick: a swing reads better a little slower than a slide (C10), so a
 * door takes eighteen ticks, about half a second, to open or shut.
 */
export const BOX_STEP = 1 / 18;

/**
 * How close to a police box's front the player must come for its open
 * doors to carry the player through, in metres (C11).
 */
export const BOX_REACH = 0.6;

/**
 * Half the clear opening of a police box's doors, in metres: the player
 * walks in only through the doorway, never through the box's side (C11).
 */
export const BOX_OPENING = 0.45;

/** A police box's doors are open enough to walk through above this fraction (C11). */
export const BOX_OPEN_ENOUGH = 0.9;

/**
 * A police box's front wall, in world metres, in `wallPoint`'s own terms:
 * the centre of the front face at floor level (the local point `(0, 1.3)`
 * through `turnedPoint`, the same point `heroFootprint`'s outer face sits
 * on), `inward` the hero's front direction (`HERO_FRONT[heroTurn(h)]`,
 * pointing away from the box into the room) and `along` the direction
 * across it, in the same terms `wallPoint` gives a fixture's wall.
 */
export function boxFront(h: Hero): WallPoint {
  const turn = heroTurn(h);
  const [fx, fz] = HERO_FRONT[turn] ?? [0, -1];
  const p = turnedPoint(h.x, h.y, turn, 0, 1.3);
  return { x: p.x, z: p.z, inward: [fx, fz], along: [fz, -fx] };
}

/**
 * Where the player stands relative to a police box's front wall (see
 * `boxFront`): `depth` metres in front of it (negative behind it, inside
 * the box's own body) and `side` metres along it from its middle.
 */
function relative(w: WallPoint, x: number, z: number) {
  const dx = x - w.x;
  const dz = z - w.z;
  return {
    depth: dx * w.inward[0] + dz * w.inward[1],
    side: dx * w.along[0] + dz * w.along[1],
  };
}

/**
 * A police box's door state, keyed by its index in `room.heroes`: the
 * state of both leaves together, in `interact.ts`'s own `DoorState`. A box
 * with no entry in the map stands shut.
 */
export type BoxDoors = ReadonlyMap<number, DoorState>;

/**
 * The police box the player is facing and can use, or null: the box
 * version of `focusOf`.
 *
 * Of every police box in `room.heroes`, the nearest one whose front
 * (`boxFront`) is within `REACH`, with the player in front of it (never
 * behind, inside the box's own body) and within `FACING` of the view
 * direction. `prompt` is `SPACE CLOSE` when that box's doors are heading
 * open (`target` 1), else `SPACE OPEN`: like a bulkhead door, it says only
 * which way Space sends them, since the box leads nowhere a label could
 * name.
 */
export function boxFocus(
  room: RoomSpec,
  player: Player,
  boxes: BoxDoors,
): { index: number; prompt: string } | null {
  const fx = -Math.sin(player.yaw);
  const fz = -Math.cos(player.yaw);
  const cosFacing = Math.cos(FACING);
  let best: { index: number; prompt: string } | null = null;
  let bestDistance = Infinity;
  for (const [index, h] of room.heroes.entries()) {
    if (h.kind !== "police-box") continue;
    const w = boxFront(h);
    if (relative(w, player.x, player.z).depth <= 0) continue;
    const dx = w.x - player.x;
    const dz = w.z - player.z;
    const distance = Math.hypot(dx, dz);
    if (distance > REACH || distance >= bestDistance) continue;
    if (distance > 0 && (dx * fx + dz * fz) / distance < cosFacing) continue;
    const target = boxes.get(index)?.target ?? 0;
    best = { index, prompt: target === 1 ? "SPACE CLOSE" : "SPACE OPEN" };
    bestDistance = distance;
  }
  return best;
}

/**
 * Every police box's doors one tick later: the box version of `stepDoors`.
 *
 * Every police box in `room.heroes` gets a state (absent: shut). `pressed`,
 * the hero index Space was pressed at this tick, turns that box's doors
 * round; every other box keeps heading where it was, and so does the one
 * pressed once it has turned: the doors never close by themselves. Then
 * each moves `BOX_STEP` towards where it is heading, snapping within 1e-9
 * of it. Returns a new map; `boxes` is left as it was.
 */
export function stepBoxDoors(
  room: RoomSpec,
  boxes: BoxDoors,
  pressed: number | null,
): Map<number, DoorState> {
  const out = new Map<number, DoorState>();
  room.heroes.forEach((h, index) => {
    if (h.kind !== "police-box") return;
    const was = boxes.get(index) ?? { open: 0, target: 0 };
    const target: 0 | 1 =
      pressed === index ? (was.target === 1 ? 0 : 1) : was.target;
    const open =
      target === 1
        ? Math.min(1, was.open + BOX_STEP)
        : Math.max(0, was.open - BOX_STEP);
    out.set(index, {
      open: Math.abs(open - target) < 1e-9 ? target : open,
      target,
    });
  });
  return out;
}

/**
 * The police box the player is walking into through its open doors, or
 * null: the box version of `travelOf`, but of the box's own doors, not a
 * way out of the room.
 *
 * Once a box's doors stand more than `BOX_OPEN_ENOUGH` open, the player's
 * centre in front of its front wall (`boxFront`), within `BOX_REACH` of it
 * and within `BOX_OPENING` of its centre line.
 */
export function boxEntry(
  room: RoomSpec,
  player: Player,
  boxes: BoxDoors,
): number | null {
  for (const [index, h] of room.heroes.entries()) {
    if (h.kind !== "police-box") continue;
    if ((boxes.get(index)?.open ?? 0) <= BOX_OPEN_ENOUGH) continue;
    const r = relative(boxFront(h), player.x, player.z);
    if (r.depth >= 0 && r.depth < BOX_REACH && Math.abs(r.side) < BOX_OPENING) {
      return index;
    }
  }
  return null;
}

/**
 * One row of the domain listing, as the console room's exit reads it: the
 * three fields `pickExitDomain` and the room-left match need. `name` is the
 * local registered name (the exit's own key, as `IDCLEV` keys and jumps to
 * a domain by its local name too); `canonicalName` and `aliases` are the
 * other spellings the same domain answers to.
 *
 * Defined on the world side, not the data layer, so the world never has to
 * import from `data/`: `data/source.ts` imports this type from here instead.
 */
export interface DomainRow {
  /** The local registered name: the row's key and where a walk out lands. */
  name: string;
  /** The name the domain's content declares, or null when not said. */
  canonicalName: string | null;
  /** Every former name the domain still answers to. */
  aliases: readonly string[];
}

/**
 * The seed of one exit pick: `seedFor("box-exit", from, ticks)`, `from` the
 * local name of the domain the room was entered from and `ticks` the
 * session's own tick count at the moment of the exit. Deterministic under a
 * hand-cranked clock in a test and different on every visit in play, since
 * the tick count never repeats: no wall clock, no counter of its own (2.6e
 * C13).
 */
export function exitSeed(from: string, ticks: number): number {
  return seedFor("box-exit", from, ticks);
}

/**
 * The one local name the spelling `from` means among `rows`, by the very
 * table links resolve through (`domainSpellings`): a local name always
 * wins, a canonical name or an alias maps to its domain unless another row's
 * local name already holds that same text (it is shadowed) or two-plus rows
 * claim it (it is contested and resolves nowhere). `from` itself when no
 * row's local name, canonical name or alias maps it at all.
 *
 * Resolving through the one shared table, rather than checking each row's
 * three fields independently, is what keeps a naming collision from
 * over-excluding: a row whose *canonical name text* happens to equal
 * another row's *local name* must not match on that text, since the
 * spelling belongs to the other domain (2.6e review: `answersTo` matched
 * every row whose canonical name or alias happened to equal `from`, so two
 * rows sharing a contested canonical name, or a row's canonical name
 * shadowed by another's local name, were both excluded even though at most
 * one of them is truly the domain `from` names).
 */
function resolveLocal(rows: readonly DomainRow[], from: string): string {
  const spellings = domainSpellings(
    rows.map((row) => ({
      name: row.name,
      canonicalName: row.canonicalName,
      aliases: [...row.aliases],
    })),
  );
  return spellings.get(from) ?? from;
}

/**
 * Which domain the console room's inner doors lead out to: one of `rows`
 * chosen uniformly at random among every row other than the one the room
 * was entered from, `from` resolved to its one local name first
 * (`resolveLocal`), so an alias or a canonical name spelled in the door
 * that led in still excludes that same domain, and only that domain, even
 * across a naming collision elsewhere in the listing (Review Focus 1).
 *
 * The rows are sorted by their local name before the draw, so the pick
 * does not depend on the listing's own order: two listings holding the same
 * domains in a different order draw the same domain for the same seed.
 *
 * With no other domain to pick, whether because `rows` holds only the room
 * left's own row or none at all, the room left's own domain is given back:
 * the resolved local name itself, when some row is registered under it, or
 * `from` when none is (an empty listing, C13's fallback).
 *
 * `seed` is `exitSeed(from, ticks)`; a fresh `createRng(seed)` is drawn from
 * once, so the same seed always gives the same pick.
 */
export function pickExitDomain(
  rows: readonly DomainRow[],
  from: string,
  seed: number,
): string {
  const local = resolveLocal(rows, from);
  const sorted = [...rows].sort((a, b) =>
    a.name < b.name ? -1 : a.name > b.name ? 1 : 0,
  );
  const others = sorted.filter((row) => row.name !== local);
  if (others.length === 0) {
    return sorted.find((row) => row.name === local)?.name ?? from;
  }
  const index = Math.floor(createRng(seed).next() * others.length);
  return others[index]?.name ?? from;
}
