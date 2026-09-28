/**
 * The game's addresses, mirrored from Fluid's under `π`, the Unicode
 * character, deliberately hard to type: this is an Easter egg, meant to be
 * reached from the C64 screen rather than typed in by hand, and there is no
 * `/game` alias or redirect into it any more.
 *
 * Every place in the station stands for a page the app already has: the
 * airlock for the front page, a bridge for its domain's page, a deck for a
 * folder (`?path=`) and an engram's room for its reading page
 * (`/d/<domain>/e/<permalink>`). The game does not invent a second scheme
 * beside those. It puts the same route under a `π` prefix (a deck adds only
 * its `section`), so a place converts to its Fluid page and back by adding
 * or dropping that prefix, with no lookup table and no second encoder that
 * could drift from the first. The F key's "open this in Fluid" and the
 * arrival after a reload both rest on that.
 *
 * A domain's bridge, the room built from its MANIFEST, has the station
 * address `bridgeAddress` names and the domain's own route under the
 * prefix. The level cheat's jump and the console room's exit go there.
 *
 * `StationAddress` (M3 C1) widens this to every place the game can put a
 * player: the airlock and a deck have neither, so `PlaceAddress` alone
 * cannot name them. `gameRouteOf` and `fluidRouteOfStation` build a station
 * address's game and Fluid routes the same way `gameEngramRoute` builds an
 * engram's; `addressOfGameLocation` reads a game location back into one, the
 * `PlaceAddress` sibling of `fluidRouteOf`. `addressOfRoute` is the same
 * reading for a plain Fluid engram route, used both here and by
 * `data/place.ts`'s reference resolver.
 */

import { domainRoute, engramRoute, folderRoute } from "../paths";
import type { PlaceAddress, StationAddress } from "./world/types";

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
 * `session.ts` compares a route `gameRouteOf` builds against
 * `window.location.pathname + window.location.search` directly, replacing
 * the URL only when the two differ, and both browsers and jsdom always
 * report that pathname percent-encoded, never as the raw character, which
 * `paths.test.ts` pins with a failing-raw-form test. A prefix built from the raw character
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
 * The station address of a domain's bridge (M3 C3), whose game route is
 * the domain's own, `/π/d/<domain>`: where the level cheat's jump
 * (`Session.jump`) and the console room's exit go.
 */
export function bridgeAddress(domain: string): StationAddress {
  return { kind: "bridge", domain };
}

/**
 * Read an engram route (`/d/<domain>/e/<permalink>`) back into its address,
 * or null when it is not one.
 *
 * Split into segments and read by position rather than searched for `/e/`, so
 * a domain that happens to be called `e` still reads right. Every segment is
 * decoded on its own, the inverse of how `engramRoute` encoded it.
 */
export function addressOfRoute(href: string): PlaceAddress | null {
  const [lead, d, domain, e, ...rest] = href.split("/");
  if (
    lead !== "" ||
    d !== "d" ||
    e !== "e" ||
    domain === undefined ||
    domain === "" ||
    rest.length === 0
  ) {
    return null;
  }
  try {
    return {
      domain: decodeURIComponent(domain),
      permalink: rest.map((segment) => decodeURIComponent(segment)).join("/"),
    };
  } catch {
    return null;
  }
}

/**
 * The game route of a station address (M3 C2), under the same `π` prefix
 * `gameEngramRoute` builds: for an engram it is `gameEngramRoute` itself,
 * since both defer to `engramRoute`.
 *
 * A deck's `folder` goes through `folderRoute`, which already encodes it
 * with `encodeURIComponent` and writes `?path=<folder>` for every folder but
 * the root; the root deck has no folder for `folderRoute` to write a
 * `?path=` for (it returns the bare domain route there), so that one case is
 * built by hand instead. A resolved section above the first is appended as
 * `&section=<n+1>`, 1-based like the rest of the app's URLs, which never
 * carry a 0; a deck at its first section or with an unresolved one (`null`,
 * C1) writes none, since both land on the same page.
 *
 * `URLSearchParams` is never used to write here: it spells a space `+`, not
 * the `%20` `folderRoute` chose, and C5's landing compares pathname and
 * search verbatim - the two spellings would never agree, and the session
 * would replace the URL on every landing instead of only when it must.
 */
export function gameRouteOf(address: StationAddress): string {
  switch (address.kind) {
    case "airlock":
      return GAME_PREFIX;
    case "bridge":
      return `${GAME_PREFIX}${domainRoute(address.domain)}`;
    case "deck": {
      const base =
        address.folder === ""
          ? `${domainRoute(address.domain)}?path=`
          : folderRoute(address.domain, address.folder);
      const section =
        address.section !== null && address.section > 0
          ? `&section=${String(address.section + 1)}`
          : "";
      return `${GAME_PREFIX}${base}${section}`;
    }
    case "engram":
      return `${GAME_PREFIX}${engramRoute(address.domain, address.permalink)}`;
  }
}

/**
 * The Fluid route a station address mirrors (M3 C2): the same builders
 * `gameRouteOf` defers to, minus the `π` prefix, minus a section (Fluid has
 * no notion of one) and minus the hand-written `?path=` for a deck's root
 * folder, since `folderRoute` already answers the bare domain route there.
 */
export function fluidRouteOfStation(address: StationAddress): string {
  switch (address.kind) {
    case "airlock":
      return "/";
    case "bridge":
      return domainRoute(address.domain);
    case "deck":
      return folderRoute(address.domain, address.folder);
    case "engram":
      return engramRoute(address.domain, address.permalink);
  }
}

/**
 * The station address a game location names, or null when the pathname is
 * not under the `π` prefix at all - the `/π/*` route would not have matched
 * it, so this case is only reachable from a direct history change.
 *
 * `fluidRouteOf` strips the prefix; what is left is read as `/` (the
 * airlock), `/d/<domain>` on its own (the bridge, or a deck when the search
 * carries a `path` parameter - however it decodes, including empty, C2's
 * root deck), `/d/<domain>/e/<permalink...>` through `addressOfRoute` (an
 * engram), or anything else under the prefix, which is the airlock too - the
 * catch-all for a typo or a route this version does not know (`/π/foo`).
 * `search` is read with `URLSearchParams`, safe on the reading side even
 * though `gameRouteOf` never writes with it (see there): `section` reads a
 * positive integer 1-based and stores it 0-based; anything else, including a
 * literal `0` or no `section` at all, reads the first (C1).
 */
export function addressOfGameLocation(
  pathname: string,
  search: string,
): StationAddress | null {
  const rest = fluidRouteOf(pathname);
  if (rest === pathname) {
    return null;
  }
  if (rest === "/") {
    return { kind: "airlock" };
  }
  const domainOnly = /^\/d\/([^/]+)$/.exec(rest);
  if (domainOnly !== null) {
    const segment = domainOnly[1] ?? "";
    let domain: string;
    try {
      domain = decodeURIComponent(segment);
    } catch {
      return { kind: "airlock" };
    }
    const params = new URLSearchParams(search);
    if (!params.has("path")) {
      return { kind: "bridge", domain };
    }
    const folder = params.get("path") ?? "";
    const raw = params.get("section");
    const parsed = raw === null ? Number.NaN : Number(raw);
    const section = Number.isInteger(parsed) && parsed > 0 ? parsed - 1 : 0;
    return { kind: "deck", domain, folder, section };
  }
  const engram = addressOfRoute(rest);
  return engram === null
    ? { kind: "airlock" }
    : { kind: "engram", domain: engram.domain, permalink: engram.permalink };
}

/**
 * Whether two station addresses name the same place. `null` (a session that
 * has not entered anywhere yet) never matches. A deck's `section` is
 * compared with `===`, so an unresolved deck (`section: null`, C1) never
 * reads equal to a resolved one at section 0 - the resolution still has to
 * run before the two can agree.
 */
export function sameStation(
  a: StationAddress | null,
  b: StationAddress,
): boolean {
  if (a === null || a.kind !== b.kind) {
    return false;
  }
  if (a.kind === "airlock") {
    return true;
  }
  if (a.kind === "bridge" && b.kind === "bridge") {
    return a.domain === b.domain;
  }
  if (a.kind === "deck" && b.kind === "deck") {
    return (
      a.domain === b.domain && a.folder === b.folder && a.section === b.section
    );
  }
  if (a.kind === "engram" && b.kind === "engram") {
    return a.domain === b.domain && a.permalink === b.permalink;
  }
  return false;
}

/**
 * An address in its canonical form (M3 C3): an engram address whose
 * permalink names a domain's root MANIFEST (the same case-insensitive,
 * root-only rule `DomainNav`'s `isPinnedManifest` pins a listing row with,
 * and `world/folders.ts`'s `isManifestPermalink` repeats for the tree) becomes
 * the domain's bridge instead. Every other address is returned unchanged.
 *
 * The rule is spelled out again here rather than imported from
 * `folders.ts`, so `paths.ts` - which the session, the routes and the data
 * layer all reach into - never has to pull in the folder and deck math the
 * generator side needs.
 */
export function canonicalStation(address: StationAddress): StationAddress {
  if (
    address.kind === "engram" &&
    address.permalink.toUpperCase() === MANIFEST_PERMALINK.toUpperCase()
  ) {
    return { kind: "bridge", domain: address.domain };
  }
  return address;
}

/**
 * The canonical station address of a `PlaceAddress`: an ordinary engram
 * room, unless C3's MANIFEST rule turns it into the domain's bridge.
 */
export function stationOfPlace(place: PlaceAddress): StationAddress {
  return canonicalStation({
    kind: "engram",
    domain: place.domain,
    permalink: place.permalink,
  });
}

/** The domain a station address names, or null for the airlock, which names none. */
export function domainOf(address: StationAddress): string | null {
  return address.kind === "airlock" ? null : address.domain;
}
