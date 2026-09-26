/**
 * The game's addresses, mirrored from Fluid's under `π`, the Unicode
 * character, deliberately hard to type: this is an Easter egg, meant to be
 * reached from the C64 screen rather than typed in by hand, and there is no
 * `/game` alias or redirect into it any more.
 *
 * Every place in the station is an engram, and every engram already has an
 * address in the app: `/d/<domain>/e/<permalink>`. The game does not invent a
 * second scheme beside it. It puts the same path under a `π` prefix, so a
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

/**
 * The prefix segment itself, as the one character it is. `routes.tsx` writes
 * this same character literally in its route patterns, and React Router
 * matches that pattern whether the browser sends the raw character or its
 * percent-encoding - so the pattern side never has to choose a spelling.
 * Building a URL to hand to `navigate` is a different problem, solved below.
 */
const GAME_SEGMENT = "π";

/**
 * The prefix every game route sits under, in the form `history` itself
 * settles on.
 *
 * `session.ts` compares a route this module builds against
 * `window.location.pathname` directly (`if (window.location.pathname !==
 * path) opts.navigate(path)`), and both browsers and jsdom always report
 * that pathname percent-encoded, never as the raw character - that is the
 * one fact this file's header used to only assert and `paths.test.ts` now
 * pins with a failing-raw-form test. A prefix built from the raw character
 * would therefore never string-equal what the location bar actually holds,
 * and the route would `replace` the URL on every landing instead of only
 * when it must. Encoding it here with the same `encodeURIComponent` a
 * browser's own normalization agrees with (uppercase hex, `%CF%80`) keeps
 * the two sides byte-for-byte identical. A hand-typed `/%cf%80/...` still
 * matches on the way in (`fluidRouteOf` below, and the route pattern
 * itself); it only costs one harmless replace up to the uppercase form,
 * same as any other case a browser would have canonicalized for you.
 */
const GAME_PREFIX = `/${encodeURIComponent(GAME_SEGMENT)}`;

/**
 * The game's address of one engram: Fluid's engram route under the `π`
 * prefix.
 *
 * Built from `engramRoute`, so the permalink's segments are encoded one by one
 * and its slashes stay slashes, exactly as the reading screen's own links are.
 */
export function gameEngramRoute(domain: string, permalink: string): string {
  return `${GAME_PREFIX}${engramRoute(domain, permalink)}`;
}

/**
 * The Fluid route a game path mirrors: the same path without its `π`
 * prefix, and the app's root for the bare game path.
 *
 * The leading segment is decoded on its own - never the whole path, so a
 * permalink's own percent-escapes past it survive untouched - and compared
 * against the raw character. That one comparison recognises every spelling
 * a prefix can arrive in: the raw character, or its percent-encoding in
 * either letter case (`%CF%80`, the form a browser settles on, or a
 * hand-typed `%cf%80`). A segment that only starts with the same letters
 * (`/πx`, `/%CF%80x`) decodes to something else entirely and is handed back
 * as it came, the same way a malformed escape is: caught rather than thrown.
 * A path under the old `/game` prefix is not a game path any more and is
 * likewise handed back unchanged - there is no alias.
 */
export function fluidRouteOf(gamePath: string): string {
  if (!gamePath.startsWith("/")) {
    return gamePath;
  }
  const slashIndex = gamePath.indexOf("/", 1);
  const segment =
    slashIndex === -1 ? gamePath.slice(1) : gamePath.slice(1, slashIndex);
  let decoded: string;
  try {
    decoded = decodeURIComponent(segment);
  } catch {
    return gamePath;
  }
  if (decoded !== GAME_SEGMENT) {
    return gamePath;
  }
  if (slashIndex === -1) {
    return "/";
  }
  const rest = gamePath.slice(slashIndex);
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
