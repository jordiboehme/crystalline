/**
 * Turning `[[Target]]` in prose into somewhere to go.
 *
 * A wikilink is only a link where the server says what it resolves to, so this
 * app never invents a route out of the text inside the brackets. Two payloads
 * together are what make one: the engram detail says, per parsed link, whether
 * the index resolved it, and the neighborhood graph says where the resolved
 * ones actually live, because the detail payload carries the target as it was
 * written (a title, usually) and never a permalink.
 *
 * That split is why a resolver answers three things rather than two. Resolved
 * and located is a link. Resolved by the index but not yet located, which is
 * every wikilink while the graph request is still in flight, is prose: it will
 * become a link a moment later, and marking it broken in the meantime would be
 * a claim the app cannot back. Parsed and unresolved is the one honest negative,
 * and it is drawn as such.
 */

import type { EngramDetail, LinkTarget } from "./api/engram";
import type { GraphNeighborhood } from "./api/graph";
import { engramRoute } from "./paths";

/** What one `[[Target]]` turned out to be. */
export type WikilinkResolution =
  /** An engram this app can navigate to. */
  | { kind: "resolved"; href: string; label: string }
  /** A target the index looked for and did not find. */
  | { kind: "unresolved" };

/**
 * What a renderer asks about the text inside one pair of brackets. `null` means
 * nothing is known about it, which is drawn as the prose it was written as.
 */
export type WikilinkResolver = (inner: string) => WikilinkResolution | null;

/**
 * The bracket pair itself. No nesting and no empty target: `[[]]` is
 * punctuation somebody typed, not a reference.
 */
export const WIKILINK = /\[\[([^[\]]+)\]\]/g;

/**
 * Split the inside of a `[[...]]` into a domain and a target, the way the
 * server's own parser does: one leading colon group is a cross-domain prefix
 * when both sides are non-empty and the domain side has no whitespace, and
 * every further colon stays in the target text.
 */
export function parseWikiTarget(inner: string): LinkTarget {
  const text = inner.trim();
  const colon = text.indexOf(":");
  if (colon > 0) {
    const domain = text.slice(0, colon).trim();
    const rest = text.slice(colon + 1).trim();
    if (domain !== "" && rest !== "" && !/\s/.test(domain)) {
      return { domain, target: rest };
    }
  }
  return { domain: null, target: text };
}

/**
 * A parsed target written back the way it appeared inside the brackets, so a
 * relation and a prose wikilink pointing at the same place ask the resolver the
 * same question.
 */
export function innerOf(target: LinkTarget): string {
  return target.domain === null
    ? target.target
    : `${target.domain}:${target.target}`;
}

/** The key one target is looked up by: its domain, if it named one, and its text. */
function keyOf(target: LinkTarget, fallbackDomain: string): string {
  return `${(target.domain ?? fallbackDomain).toLowerCase()} ${target.target.toLowerCase()}`;
}

/**
 * Build the resolver for one engram page.
 *
 * `graph` is optional because it arrives second: the same resolver is used
 * before and after it lands, and every wikilink it cannot place yet answers
 * `null` until then.
 */
export function buildWikilinkResolver(
  detail: EngramDetail,
  graph: GraphNeighborhood | undefined,
): WikilinkResolver {
  const home = detail.domain;

  // What the index made of each parsed reference. Both lists are consulted,
  // because a target written as prose on one line and declared as a relation on
  // another is the same target, and the engram page draws the prose.
  const parsed = new Map<string, boolean>();
  for (const reference of [...detail.links, ...detail.relations]) {
    const key = keyOf(reference.target, home);
    // Resolved anywhere wins: the same text on two lines is one target, and one
    // line failing to resolve while another succeeds is an indexing detail
    // rather than something to draw twice.
    parsed.set(key, (parsed.get(key) ?? false) || reference.resolved);
  }

  // Where the neighbors live, by title and by permalink, since a wikilink may
  // be written as either.
  const located = new Map<string, { domain: string; permalink: string }>();
  for (const node of graph?.nodes ?? []) {
    const where = { domain: node.domain, permalink: node.permalink };
    for (const name of [node.title, node.permalink]) {
      // Keyed through the same function the lookup uses, so the two can never
      // disagree about what a key is.
      const key = keyOf({ domain: node.domain, target: name }, node.domain);
      if (name !== "" && !located.has(key)) {
        located.set(key, where);
      }
    }
  }

  return (inner: string) => {
    const target = parseWikiTarget(inner);
    const key = keyOf(target, home);
    const resolved = parsed.get(key);
    if (resolved === undefined) {
      // Not a reference the server parsed out of this engram at all. Rendered
      // as prose rather than guessed at: bracket text inside, say, a quoted
      // example is not a link nobody wrote.
      return null;
    }
    if (!resolved) {
      return { kind: "unresolved" };
    }
    const where = located.get(key);
    return where === undefined
      ? null
      : {
          kind: "resolved",
          href: engramRoute(where.domain, where.permalink),
          label: target.target,
        };
  };
}
