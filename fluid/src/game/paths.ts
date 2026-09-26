/**
 * The game's addresses, mirrored from Fluid's under `/game`.
 *
 * Every place in the station is an engram, and every engram already has an
 * address in the app: `/d/<domain>/e/<permalink>`. The game does not invent a
 * second scheme beside it. It puts the same path under a `/game` prefix, so a
 * place converts to its reading page and back by adding or dropping that
 * prefix, with no lookup table and no second encoder that could drift from the
 * first. The F key's "open this in Fluid" and the arrival after a reload both
 * rest on that.
 *
 * A domain's bridge, its MANIFEST room, has the address `bridgeAddress`
 * names: the domain and the permalink `MANIFEST_PERMALINK`. The level
 * cheat's jump goes there.
 */

import { engramRoute } from "../paths";
import type { PlaceAddress } from "./world/types";

/** The prefix every game route sits under. */
const GAME_PREFIX = "/game";

/**
 * The game's address of one engram: Fluid's engram route under `/game`.
 *
 * Built from `engramRoute`, so the permalink's segments are encoded one by one
 * and its slashes stay slashes, exactly as the reading screen's own links are.
 */
export function gameEngramRoute(domain: string, permalink: string): string {
  return `${GAME_PREFIX}${engramRoute(domain, permalink)}`;
}

/**
 * The Fluid route a game path mirrors: the same path without its `/game`
 * prefix, and the app's root for the bare game path.
 *
 * Only a whole `/game` segment is stripped, so a path that merely starts with
 * the same letters (`/gamer`) is handed back as it came.
 */
export function fluidRouteOf(gamePath: string): string {
  if (gamePath === GAME_PREFIX) {
    return "/";
  }
  if (!gamePath.startsWith(`${GAME_PREFIX}/`)) {
    return gamePath;
  }
  const rest = gamePath.slice(GAME_PREFIX.length);
  return rest === "/" ? "/" : rest;
}

/**
 * The key one place is known by inside the game: domain and permalink joined
 * by a NUL.
 *
 * A NUL rather than a slash because a permalink carries slashes of its own
 * and a domain could in principle too, while neither can hold a NUL; the key
 * is therefore unambiguous and sorts by domain first, then permalink.
 */
export function placeKeyOf(domain: string, permalink: string): string {
  return `${domain}\u0000${permalink}`;
}

/**
 * The permalink of a domain's MANIFEST, the engram its bridge is built
 * from: the slug the index gives `MANIFEST.md` when its frontmatter names
 * none, and the one the server's starter MANIFEST declares. A MANIFEST
 * that declares another permalink is not found under it (C9).
 */
export const MANIFEST_PERMALINK = "manifest";

/**
 * The address of a domain's bridge, its MANIFEST room: where the level
 * cheat's jump goes (`Session.jump`).
 */
export function bridgeAddress(domain: string): PlaceAddress {
  return { domain, permalink: MANIFEST_PERMALINK };
}
