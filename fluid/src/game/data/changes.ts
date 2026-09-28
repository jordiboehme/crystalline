/**
 * What one change frame means for the station (M4 C13, C14, C18). Pure:
 * the session decides when to act, this module only answers.
 *
 * `changeKeys` is the game's invalidation: every key Fluid's own table
 * (`keysFor`) names for the frame, plus, for an `engram` or a `domain`
 * frame, the prefix of the game's inbound pages for that domain
 * (`gameInboundPrefix`), the one key of the game's own that the table
 * never names. A `reset` is "everything", as in the table.
 *
 * `watchOf` says what the current place depends on and `concerns` whether a
 * frame touches it:
 *
 * - an engram room watches its own domain and every domain one of its ways
 *   leads to (a door, a portal or a hatch with an address, an exit's
 *   station address), so a cross-domain door to an engram that goes is
 *   re-checked too; a `domain` frame for any of those domains counts;
 * - a bridge and a deck watch their domain;
 * - the airlock watches the domain listing: any `domain` frame, and an
 *   `engram` frame whose keys in Fluid's table include `DOMAINS_QUERY_KEY`
 *   (an add, a delete, a MANIFEST edit), so the rule stays the table's;
 * - the console room watches nothing: the room walked in from is read
 *   fresh on its next entry, since the invalidation already made it stale;
 * - a `reset` concerns every place that watches something.
 *
 * `movedTo` follows a move of the current engram (C18): a `moved` frame in
 * the current engram's domain whose `from.permalink` is the current one
 * sends the station to the new permalink.
 */

import { DOMAINS_QUERY_KEY } from "../../api/domains";
import type { ChangeEvent } from "../../api/events";
import { keysFor } from "../../events/invalidation";
import { domainOf } from "../paths";
import type { RoomSpec, StationAddress } from "../world/types";

/**
 * The prefix of the game's own inbound pages for one domain (M4 C13):
 * `["game", "inbound", domain]`, the first three parts of `inboundKey`
 * (`data/source.ts`), so one invalidation reaches the page of every engram
 * of the domain.
 */
export function gameInboundPrefix(domain: string): readonly unknown[] {
  return ["game", "inbound", domain];
}

/**
 * Every key a frame makes stale in the game's cache: Fluid's table
 * (`keysFor`) in its own order, then the inbound prefix of the frame's
 * domain for an `engram` or `domain` frame; `"everything"` for a `reset`.
 */
export function changeKeys(
  event: ChangeEvent,
): (readonly unknown[])[] | "everything" {
  const keys = keysFor(event);
  if (keys === "everything" || event.event === "reset") return "everything";
  return [...keys, gameInboundPrefix(event.change.domain)];
}

/**
 * What the current place depends on (M4 C14): nothing (the console room,
 * or no place yet), the domain listing (the airlock), one domain (a bridge
 * or a deck), or an engram room's domain and the other domains its ways
 * lead to (`wayDomains`, sorted, each once, never the room's own).
 */
export type Watch =
  | { kind: "none" }
  | { kind: "airlock" }
  | { kind: "domain"; domain: string }
  | {
      kind: "engram";
      domain: string;
      permalink: string;
      wayDomains: readonly string[];
    };

/** The domains a room's ways lead to, with duplicates and sealed ways left out. */
function wayDomainsOf(room: RoomSpec | null, own: string): string[] {
  if (room === null) return [];
  const found = new Set<string>();
  for (const f of room.fixtures) {
    switch (f.kind) {
      case "door":
      case "portal":
      case "hatch":
        if (f.address !== null) found.add(f.address.domain);
        break;
      case "exit": {
        const domain = domainOf(f.to);
        if (domain !== null) found.add(domain);
        break;
      }
      default:
        break;
    }
  }
  found.delete(own);
  return [...found].sort();
}

/**
 * The watch for the place the player is in (M4 C14): `inside` the console
 * room, or with no address, nothing; the airlock its listing; a bridge or
 * a deck its domain; an engram room its domain and the domains of the
 * ways in `room` (none while `room` is null). See the module doc.
 */
export function watchOf(
  address: StationAddress | null,
  room: RoomSpec | null,
  inside: boolean,
): Watch {
  if (inside || address === null) return { kind: "none" };
  switch (address.kind) {
    case "airlock":
      return { kind: "airlock" };
    case "bridge":
    case "deck":
      return { kind: "domain", domain: address.domain };
    case "engram":
      return {
        kind: "engram",
        domain: address.domain,
        permalink: address.permalink,
        wayDomains: wayDomainsOf(room, address.domain),
      };
  }
}

const DOMAINS_ROW = JSON.stringify(DOMAINS_QUERY_KEY);

/**
 * True when the frame concerns the watched place (M4 C14), so the session
 * re-checks it; a frame that does not only invalidates. See the module doc.
 */
export function concerns(event: ChangeEvent, watch: Watch): boolean {
  if (watch.kind === "none") return false;
  if (event.event === "reset") return true;
  switch (watch.kind) {
    case "airlock": {
      if (event.event === "domain") return true;
      const keys = keysFor(event);
      return (
        keys === "everything" ||
        keys.some((k) => JSON.stringify(k) === DOMAINS_ROW)
      );
    }
    case "domain":
      return event.change.domain === watch.domain;
    case "engram":
      return (
        event.change.domain === watch.domain ||
        watch.wayDomains.includes(event.change.domain)
      );
  }
}

/**
 * The address a `moved` frame sends the current engram to, or null when
 * the frame is no move of it (M4 C18): the frame's domain must be the
 * current engram's and its `from.permalink` the current permalink; the
 * answer is the engram at the frame's new `permalink`.
 */
export function movedTo(
  event: ChangeEvent,
  current: StationAddress | null,
): StationAddress | null {
  if (event.event !== "engram" || current?.kind !== "engram") return null;
  const change = event.change;
  if (change.kind !== "moved" || change.from === null) return null;
  if (change.domain !== current.domain) return null;
  if (change.from.permalink !== current.permalink) return null;
  return { kind: "engram", domain: change.domain, permalink: change.permalink };
}
