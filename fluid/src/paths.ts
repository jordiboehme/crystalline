/**
 * The addresses inside the app, built rather than typed out.
 *
 * A route pattern and the links that point at it are one fact, and this is
 * where that fact lives: change a pattern in `routes.tsx` and the builders
 * beside it move with it, instead of a template literal in some component
 * quietly pointing nowhere.
 */

import { encodePermalink, encodeSegment } from "./api/client";
import { crystallineAddress } from "./api/engram";
import { NEIGHBORHOOD_DEPTH } from "./api/graph";

/**
 * Whether `path` is a domain's MANIFEST: the root file, in any letter case,
 * the way the engine's `names_the_manifest` reads it. A file domain refuses
 * to move or delete it (422 since 0.27.0).
 */
export function isDomainManifestPath(path: string): boolean {
  // ASCII-only, like the engine's `eq_ignore_ascii_case`: `toUpperCase` is
  // Unicode-aware and would turn a dotless "ı" into an "I".
  return path.replace(/[A-Z]/g, (c) => c.toLowerCase()) === "manifest.md";
}

/**
 * The address of one domain: the link that just opens it. The route decides
 * the view (`DomainEntry`): the Reflection when it is in effect here and the
 * caller may read it, the engrams otherwise.
 */
export function domainRoute(domain: string): string {
  return `/d/${encodeSegment(domain)}`;
}

/**
 * A domain's engram view: its MANIFEST, its folders and its engrams. A
 * segment of its own since 0.28.1, when a domain started to open on its
 * Reflection, so a link that means the engrams says so.
 */
export function engramsRoute(domain: string): string {
  return `${domainRoute(domain)}/engrams`;
}

/**
 * One folder of a domain, browsed on the engram view.
 *
 * A query parameter rather than a segment of the path, because the browse view
 * is one of the states that screen holds in its URL beside the frontmatter
 * filters, and the root folder is the engram view itself rather than a folder
 * named nothing. An older `/d/<domain>?path=` link still lands here: the
 * domain's own address sends any query on to the engram view.
 */
export function folderRoute(domain: string, folder: string): string {
  return folder === ""
    ? engramsRoute(domain)
    : `${engramsRoute(domain)}?path=${encodeURIComponent(folder)}`;
}

/**
 * The address of one engram.
 *
 * Built from the client's own encoders rather than from its `engramPath`,
 * which is the API path (`/domains/{d}/engrams/{permalink}`) and would be the
 * wrong URL to put in a link. What matters is shared: a permalink is itself a
 * path, so its segments are encoded one by one and its slashes stay slashes,
 * which is exactly what the `/d/:domain/e/*` pattern matches. Encoding the
 * permalink whole would turn `notes/deep/gamma` into one escaped segment and
 * the link would miss.
 */
export function engramRoute(domain: string, permalink: string): string {
  return `${domainRoute(domain)}/e/${encodePermalink(permalink)}`;
}

/**
 * The editor over one engram. Not under `/e/`: that pattern ends in a splat,
 * and a splat swallows everything after it, so `edit` gets its own segment
 * ahead of the permalink - the same shape the API gave its action routes.
 */
export function editRoute(domain: string, permalink: string): string {
  return `${domainRoute(domain)}/edit/${encodePermalink(permalink)}`;
}

/**
 * A domain's Reflection: its home, where the tree, the pact and search are.
 * A segment of its own after the domain rather than a query parameter,
 * because it is a mode of the whole domain and every screen under it keeps
 * it.
 */
export function reflectionRoute(domain: string): string {
  return `${domainRoute(domain)}/reflection`;
}

/**
 * One reflection, by its path inside `reflections/` (`guides/deploying.md`,
 * `PACT.md`). The path is encoded segment by segment like a permalink, so
 * its slashes stay slashes and the splat matches it.
 */
export function reflectionPageRoute(domain: string, path: string): string {
  return `${reflectionRoute(domain)}/${encodePermalink(path)}`;
}

/** The live editor over one reflection; `reflection-edit` ahead of the splat, as `edit` is. */
export function reflectionEditRoute(domain: string, path: string): string {
  return `${domainRoute(domain)}/reflection-edit/${encodePermalink(path)}`;
}

/**
 * The review screen: open threads and the reflect queue. Static, so it
 * outranks the page splat; a page path always ends in `.md`, so no page can
 * be called this.
 */
export function reflectionReviewRoute(domain: string): string {
  return `${reflectionRoute(domain)}/review`;
}

/**
 * Reflection-mode search, on the Reflection home. The mode (`search_type`)
 * and the page ride in the address only when they are not the defaults, so a
 * result page is a link somebody can send.
 */
export function reflectionSearchRoute(
  domain: string,
  query: string,
  mode = "hybrid",
  page = 1,
): string {
  const q = new URLSearchParams({ q: query });
  if (mode !== "hybrid") {
    q.set("search_type", mode);
  }
  if (page > 1) {
    q.set("page", String(page));
  }
  return `${reflectionRoute(domain)}?${q.toString()}`;
}

/**
 * The reflection path from the `/d/:domain/*` splat: `""` on the Reflection
 * home and review screen, null outside the Reflection. Here rather than in
 * the sidebar tree so the frame can ask without loading the tree.
 */
export function reflectionPathOf(rest: string): string | null {
  if (rest === "reflection" || rest === "reflection/review") {
    return "";
  }
  if (rest.startsWith("reflection/")) {
    return decodeURIComponent(rest.slice("reflection/".length));
  }
  if (rest.startsWith("reflection-edit/")) {
    return decodeURIComponent(rest.slice("reflection-edit/".length));
  }
  return null;
}

/**
 * The route is kept as the base of the editor's address and redirects to
 * the engram view, where the MANIFEST is now read.
 */
export function manifestRoute(domain: string): string {
  return `${domainRoute(domain)}/manifest`;
}

/** The editor over one domain's MANIFEST. */
export function manifestEditRoute(domain: string): string {
  return `${manifestRoute(domain)}/edit`;
}

/**
 * A domain's settings: members, review mode, policies, backup and the danger
 * zone, off the front page so the MANIFEST reads as the page. `part` is the
 * block a link opens at (`members`, `policies`, ...), as a fragment.
 */
export function domainSettingsRoute(domain: string, part?: string): string {
  const base = `${domainRoute(domain)}/settings`;
  return part === undefined ? base : `${base}#${part}`;
}

/**
 * What the domain page asks the MANIFEST editor to write into the buffer, in
 * the navigation state of the hop between them.
 *
 * A name rather than the text itself: the text is the server's, read by the
 * editor off the manifest it loads, so the two screens never carry a copy of
 * the grammar between them.
 */
export interface SeedRequest {
  /** An H2 heading the server offers a starter for, or `WHOLE_MANIFEST`. */
  seedSection: string;
}

/**
 * The seed name that means the whole document rather than one section of it.
 *
 * Not a heading the registry knows, which is the point: the editor reads it
 * as "put the whole scaffold in", which is what a domain whose MANIFEST is
 * blank needs. Spelled here, beside the route it travels on, so the two ends
 * of one navigation cannot drift apart.
 */
export const WHOLE_MANIFEST = "MANIFEST";

/**
 * The neighborhood of one engram, full screen.
 *
 * The anchor is the engram's own `crystalline://` address rather than the two
 * halves of it, because that is what `GET /graph` takes and what somebody can
 * paste in from anywhere else. The depth rides along only when it is not the
 * default: a URL says what was chosen, and one hop is what a neighborhood is
 * when nobody chose.
 */
export function graphRoute(
  domain: string,
  permalink: string,
  depth: number = NEIGHBORHOOD_DEPTH,
): string {
  const anchor = encodeURIComponent(crystallineAddress(domain, permalink));
  return depth === NEIGHBORHOOD_DEPTH
    ? `/graph?anchor=${anchor}`
    : `/graph?anchor=${anchor}&depth=${String(depth)}`;
}

/**
 * The Administration area, which opens on its General page.
 *
 * Every page an admin uses to look after the instance lives under it:
 * General, Users, Agents, Invites and the GitHub connection. Maintenance is
 * not one of them, since every role may open it; the area only links to it.
 */
export function adminRoute(): string {
  return "/admin";
}

/** The Administration area's first page: the version, updates and address. */
export function adminGeneralRoute(): string {
  return "/admin/general";
}

/**
 * The account administration page.
 *
 * A constant rather than a literal at each call site for the reason every
 * builder here exists: the pattern in `routes.tsx` and the links pointing at
 * it are one fact, kept in one place. It was `/users` before 0.35.0, and that
 * address still leads here.
 */
export function usersRoute(): string {
  return "/admin/users";
}

/** The agents page of the Administration area. */
export function adminAgentsRoute(): string {
  return "/admin/agents";
}

/** The invites page of the Administration area. */
export function adminInvitesRoute(): string {
  return "/admin/invites";
}

/**
 * The maintenance report: what the knowledge needs next, across every domain.
 *
 * Named for the question it answers rather than for the sweep behind it. The
 * endpoint is `/evolve` and the MCP tool an agent works the queue with is
 * called `evolve` too, but a person arriving at this address came to find out
 * what is owed, and the word for that is maintenance.
 */
export function maintenanceRoute(): string {
  return "/maintenance";
}

/**
 * The GitHub connection page of the Administration area.
 *
 * It was `/settings/github` before 0.35.0, and that address still leads
 * here, with its query, so a link GitHub or a person kept keeps working.
 */
export function githubSettingsRoute(): string {
  return "/admin/github";
}

/**
 * Your own profile: who this session is, and the GitHub identity it shares as.
 *
 * Not under `/admin/`, which is where this instance's own settings live and
 * where only an admin goes. This screen is about the person rather than about
 * the machine, and everybody who can share has one.
 */
export function profileRoute(): string {
  return "/profile";
}

/** Where the topbar's search box sends a query. */
export function searchRoute(query: string): string {
  return `/search?q=${encodeURIComponent(query)}`;
}

/**
 * Everything carrying one tag, across every domain.
 *
 * Search rather than a domain listing on purpose: a tag is a thread through the
 * whole knowledge base, and following it out of the domain it was noticed in is
 * the point of clicking one.
 */
export function tagRoute(tag: string): string {
  return `/search?tags=${encodeURIComponent(tag)}`;
}
