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
 *
 * A prefix may spell its domain with the local name, the canonical name the
 * domain's MANIFEST declares or a former name, and the server binds all three.
 * So the domain list is the spelling table from `domainNames.ts`, and every
 * key below names the domain by the local name it maps to, spelled exactly as
 * the server matches it.
 */

import type { EngramDetail, LinkTarget } from "./api/engram";
import type { GraphNeighborhood } from "./api/graph";
import type { DomainSpellings } from "./domainNames";
import { headingSlug } from "./anchors";
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
 * Split the inside of a `[[...]]` at its first `|` into the target part and
 * the display text, both trimmed. An empty display text is no display text.
 * The server splits the same way (`split_alias` in `core/src/engram.rs`).
 */
function splitAlias(inner: string): { target: string; alias?: string } {
  const bar = inner.indexOf("|");
  if (bar < 0) {
    return { target: inner.trim() };
  }
  const alias = inner.slice(bar + 1).trim();
  const target = inner.slice(0, bar).trim();
  return alias === "" ? { target } : { target, alias };
}

/**
 * Split a target part (the text before its `|`) at its first `#` into the
 * part before it and the heading after it, both as written. No split, and no
 * heading, when there is no `#` or nothing but whitespace stands before it
 * (`[[#setup]]`). The server splits the same way (`split_fragment` in
 * `core/src/engram.rs`).
 */
function splitFragment(part: string): { before: string; fragment?: string } {
  const hash = part.indexOf("#");
  if (hash < 0 || part.slice(0, hash).trim() === "") {
    return { before: part };
  }
  return { before: part.slice(0, hash), fragment: part.slice(hash + 1) };
}

/**
 * The domain prefix of a trimmed target part: one leading colon group is a
 * prefix when both sides are non-empty and the domain side has no whitespace,
 * and every further colon stays in the target text. The server reads it the
 * same way (`read_prefix` in `core/src/engram.rs`).
 */
function readPrefix(part: string): { domain: string | null; target: string } {
  const colon = part.indexOf(":");
  if (colon >= 0) {
    const prefix = part.slice(0, colon).trim();
    const after = part.slice(colon + 1).trim();
    if (prefix !== "" && after !== "" && !/\s/.test(prefix)) {
      return { domain: prefix, target: after };
    }
  }
  return { domain: null, target: part };
}

/**
 * Split the inside of a `[[...]]` into a domain, a target, a heading and a
 * display text, in the order the server's own parser takes
 * (`LinkTarget::parse` in `core/src/engram.rs`): the display text is what
 * follows the first `|`; the heading is what follows the first `#` of the part
 * before it, when something stands before that `#`; the domain prefix is read
 * on the part before the `#` only, so a heading may hold a colon
 * (`[[b#FAQ: General]]`). An empty heading is no heading. `whole` is the part
 * with its `#` kept, set when a `#` was split off and the reading without the
 * split names the same domain. `[[|b]]` names no target and is no link: its
 * target comes back empty.
 */
export function parseWikiTarget(inner: string): LinkTarget {
  const { target: part, alias } = splitAlias(inner);
  const withAlias = alias === undefined ? {} : { alias };
  const { before, fragment } = splitFragment(part);
  if (fragment === undefined) {
    return { ...readPrefix(part), ...withAlias };
  }
  const link = readPrefix(before.trim());
  const unsplit = readPrefix(part);
  const heading = fragment.trim();
  return {
    ...link,
    ...(heading === "" ? {} : { fragment: heading }),
    ...(unsplit.domain === link.domain ? { whole: unsplit.target } : {}),
    ...withAlias,
  };
}

/**
 * A parsed target written back the way it appeared inside the brackets, so a
 * relation and a prose wikilink pointing at the same place ask the resolver the
 * same question. A target that kept its `#` (`whole`) is written with it, so
 * `[[C# Basics]]` comes back with its space.
 */
export function innerOf(target: LinkTarget): string {
  const base =
    target.whole ??
    (target.fragment === undefined
      ? target.target
      : `${target.target}#${target.fragment}`);
  const written = target.domain === null ? base : `${target.domain}:${base}`;
  return target.alias === undefined ? written : `${written}|${target.alias}`;
}

/**
 * The words a reader sees for one bracket text when there is nothing to
 * resolve it against: the display text, or the target without one. `null`
 * when there is no target, so the bracket text stays as it was written.
 */
export function wikilinkWords(inner: string): string | null {
  const parsed = parseWikiTarget(inner);
  if (parsed.target === "") {
    return null;
  }
  return (
    parsed.alias ??
    parsed.whole ??
    (parsed.fragment === undefined
      ? parsed.target
      : `${parsed.target}#${parsed.fragment}`)
  );
}

/**
 * The key one target is looked up by: its domain, if it named one, and its
 * text. The domain is the local name its spelling maps to, compared exactly as
 * the server compares it; only the text is folded.
 */
function keyOf(
  target: LinkTarget,
  fallbackDomain: string,
  names: DomainSpellings | undefined,
): string {
  const domain =
    target.domain === null
      ? fallbackDomain
      : (names?.get(target.domain) ?? target.domain);
  return `${domain} ${target.target.toLowerCase()}`;
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
  known: DomainSpellings | undefined,
): LinkTarget[] {
  const parsed = parseWikiTarget(inner);
  // A title that holds a `#` (`C# Basics`) is tried as a whole first, the way
  // the server does, before the heading split.
  const first: LinkTarget[] =
    parsed.whole === undefined
      ? []
      : [{ domain: parsed.domain, target: parsed.whole }];
  if (
    parsed.domain === null ||
    known === undefined ||
    known.has(parsed.domain)
  ) {
    return [...first, parsed];
  }
  // The fallback reads the text before the `#`, as the server's does
  // (`LinkTarget::raw`), and keeps the heading.
  const { before } = splitFragment(splitAlias(inner).target);
  const fallback: LinkTarget = { domain: null, target: before.trim() };
  if (parsed.fragment !== undefined) {
    fallback.fragment = parsed.fragment;
  }
  return [...first, parsed, fallback];
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
  domains?: DomainSpellings,
): WikilinkResolver {
  const home = detail.domain;
  // Undefined rather than empty when the caller has no listing to give: an
  // empty table would say every prefix names no domain, which is a claim,
  // where undefined says this caller cannot tell and asks for the old behavior.
  const known = domains;

  // What the index made of each parsed reference. Both lists are consulted,
  // because a target written as prose on one line and declared as a relation on
  // another is the same target, and the engram page draws the prose.
  const parsed = new Map<string, boolean>();
  for (const reference of [...detail.links, ...detail.relations]) {
    const key = keyOf(reference.target, home, known);
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
      // disagree about what a key is. A node's domain is already the local
      // name, so it skips the spelling table: another domain's canonical name
      // or alias may be spelled like it, and must not move the node there.
      const key = keyOf(
        { domain: node.domain, target: name },
        node.domain,
        undefined,
      );
      if (name !== "" && !located.has(key)) {
        located.set(key, where);
      }
    }
  }

  // A resolved relative note link carries its address in the detail payload,
  // since its path is neither a title nor a permalink the graph could place.
  // It is labelled with the title the graph gives that address, when it has
  // one yet.
  for (const reference of [...detail.links, ...detail.relations]) {
    const permalink = reference.permalink;
    const key = keyOf(reference.target, home, known);
    if (permalink === undefined || located.has(key)) {
      continue;
    }
    const node = graph?.nodes.find(
      (n) => n.domain === home && n.permalink === permalink,
    );
    located.set(key, { domain: home, permalink, title: node?.title ?? "" });
  }

  return (inner: string) => {
    const candidates = readings(inner, known);
    const { alias, target: written } = parseWikiTarget(inner);
    if (written === "") {
      // `[[|b]]` names no target, so it is no link.
      return null;
    }

    // What the index made of it, taken from the first reading it has anything
    // to say about. A verdict of false is the verdict: a second reading is a
    // different way of asking the same server, not a second opinion.
    const resolved = candidates
      .map((target) => parsed.get(keyOf(target, home, known)))
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
      const where = located.get(keyOf(target, home, known));
      if (where !== undefined) {
        // Only a split reading carries a heading; a link located by its
        // whole text opens the engram itself.
        const fragment = target.fragment;
        const base = engramRoute(where.domain, where.permalink);
        const shown = where.title === "" ? target.target : where.title;
        return {
          kind: "resolved",
          href:
            fragment === undefined ? base : `${base}#${headingSlug(fragment)}`,
          // The engram's own title, falling back to the bracket text for a node
          // that carries none. A link written by title is labelled with that
          // title in its canonical spelling; a link written by permalink is
          // labelled with the title too, which is the whole reason the engine
          // may write the address without costing a reader the name.
          // The author's display text wins over both: `[[a|b]]` reads "b".
          label:
            alias ??
            (fragment === undefined ? shown : `${shown} › ${fragment}`),
        };
      }
    }
    return null;
  };
}
