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
 *
 * A reference is labelled with the title of the engram it lands on, not with
 * the text inside its brackets. Those are usually the same string and the
 * difference is the point when they are not: every relation the engine writes
 * for itself names the permalink, because a permalink is the stable identity
 * and never carries a colon, and a reader of a retired engram should still see
 * "Log: Weekly Garden Notes" rather than `log-weekly`. The file carries the
 * address, a person keeps seeing the name. Nothing is invented: the title comes
 * from the graph node the link resolved to, so a reference with no address yet
 * is not labelled at all - it is the prose it was written as.
 *
 * One thing the bracket text cannot settle on its own, and the server's
 * resolver cannot either: `[[Log: Weekly Garden Notes]]` splits exactly like
 * `[[ops:Runbook]]`, and only the domain registry says which of the two words
 * before the colon is a domain. So the split below stays domain-agnostic - it
 * mirrors the server's parser, and the two must not drift - and the fallback
 * lives in the resolver, which is handed the domain list this app already
 * holds. That mirrors where the server keeps it too (`core/src/address.rs`).
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
 * How a reference presents itself. Three states, and the middle one is the
 * whole reason this type exists: a reference the index resolved and the graph
 * has not placed yet is neither a link nor a broken one, and every surface that
 * draws a reference has to agree about that or the same target will read as
 * broken in one place and fine in another.
 */
export type ReferenceState =
  /** There is somewhere to go. */
  | "resolved"
  /** It goes somewhere; which somewhere is still in flight. */
  | "pending"
  /** The index looked for it and found nothing. */
  | "unresolved";

/**
 * Which of the three a reference is in.
 *
 * The single definition of the rule, so the body, the relation list and the
 * lifecycle banner cannot drift apart. `parsedResolved` is what the detail
 * payload said about this reference, for a caller holding a parsed one; a
 * caller with only bracket text in hand passes `null`, and an unrecognized
 * bracket reads as pending, which is drawn as the prose it already was.
 */
export function referenceState(
  resolution: WikilinkResolution | null,
  parsedResolved: boolean | null,
): ReferenceState {
  if (resolution !== null) {
    return resolution.kind;
  }
  return parsedResolved === false ? "unresolved" : "pending";
}

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
 * The readings of one bracket text, in the order they are tried.
 *
 * Almost always one: what the parser made of it. A second is added only when
 * the parser found a prefix and the caller's domain list says nothing is
 * registered under that name - then the whole bracket text, colon and all, is
 * a title in the engram's own domain. The cross-domain reading is still tried
 * first, so a prefix naming a real domain never loses to a title that merely
 * looks like one, and a caller that passes no domain list gets the single
 * reading it always got.
 */
function readings(
  inner: string,
  known: ReadonlySet<string> | undefined,
): LinkTarget[] {
  const parsed = parseWikiTarget(inner);
  if (
    parsed.domain === null ||
    known === undefined ||
    known.has(parsed.domain)
  ) {
    return [parsed];
  }
  return [parsed, { domain: null, target: inner.trim() }];
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
  domains?: readonly string[],
): WikilinkResolver {
  const home = detail.domain;
  // Undefined rather than empty when the caller has no listing to give: an
  // empty set would say every prefix names no domain, which is a claim, where
  // undefined says this caller cannot tell and asks for the old behavior.
  const known = domains === undefined ? undefined : new Set(domains);

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
  // be written as either. The title rides along because it is what a reader is
  // shown for whichever of the two the link was written with.
  const located = new Map<
    string,
    { domain: string; permalink: string; title: string }
  >();
  for (const node of graph?.nodes ?? []) {
    const where = {
      domain: node.domain,
      permalink: node.permalink,
      title: node.title,
    };
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
    const candidates = readings(inner, known);

    // What the index made of it, taken from the first reading it has anything
    // to say about. A verdict of false is the verdict: a second reading is a
    // different way of asking the same server, not a second opinion.
    const resolved = candidates
      .map((target) => parsed.get(keyOf(target, home)))
      .find((verdict) => verdict !== undefined);
    if (resolved === undefined) {
      // Not a reference the server parsed out of this engram at all. Rendered
      // as prose rather than guessed at: bracket text inside, say, a quoted
      // example is not a link nobody wrote.
      return null;
    }
    if (!resolved) {
      return { kind: "unresolved" };
    }

    // Where it lives, over every reading: the graph places an engram under its
    // own title, so a target the index resolved through the fallback reading is
    // findable under that reading and under no other.
    for (const target of candidates) {
      const where = located.get(keyOf(target, home));
      if (where !== undefined) {
        return {
          kind: "resolved",
          href: engramRoute(where.domain, where.permalink),
          // The engram's own title, falling back to the bracket text for a node
          // that carries none. A link written by title is labelled with that
          // title in its canonical spelling; a link written by permalink is
          // labelled with the title too, which is the whole reason the engine
          // may write the address without costing a reader the name.
          label: where.title === "" ? target.target : where.title,
        };
      }
    }
    return null;
  };
}
