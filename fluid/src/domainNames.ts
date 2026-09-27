/**
 * Which domain a spelling in a link means.
 *
 * A domain answers to three kinds of names: its local name (what this machine
 * registered it as, and what every route in this app carries), its canonical
 * name (the `domain_name` its MANIFEST declares) and its aliases (former
 * names). A link may spell the domain with any of them, and the server binds
 * it by the same table it builds in `core/src/names.rs` (`NameTable`). This is
 * that table's mirror, built from the domain listing, and the two must not
 * drift:
 *
 * 1. a local name always wins;
 * 2. a canonical name maps to its domain unless a local name already holds it
 *    (the claimant is shadowed) or two domains claim it and none is
 *    registered under it (then it resolves nowhere);
 * 3. an alias maps unless its spelling is taken by a local or canonical name,
 *    is a contested canonical name, or is listed by two domains.
 *
 * Matching is on the exact spelling: no case folding and no trimming.
 */

import type { DomainSummary } from "./api/domains";

/** The part of one listing row the table is built from. */
export type DomainNameRow = Pick<
  DomainSummary,
  "name" | "canonicalName" | "aliases" | "shadowed"
>;

/** Every spelling a domain answers to, mapped to its local name. */
export type DomainSpellings = ReadonlyMap<string, string>;

/** Build the spelling table from the domain listing. */
export function domainSpellings(
  rows: readonly DomainNameRow[],
): DomainSpellings {
  const spellings = new Map<string, string>();

  // 1. Local names.
  for (const row of rows) {
    spellings.set(row.name, row.name);
  }

  // 2. Canonical names that differ from the local name.
  const claims = new Map<string, string[]>();
  for (const row of rows) {
    const canonical = row.canonicalName;
    if (canonical !== null && canonical !== row.name) {
      claims.set(canonical, [...(claims.get(canonical) ?? []), row.name]);
    }
  }
  const contested = new Set<string>();
  for (const [name, claimants] of claims) {
    if (spellings.has(name)) {
      continue;
    }
    if (claimants.length > 1) {
      contested.add(name);
    } else {
      spellings.set(name, claimants[0] ?? name);
    }
  }

  // 3. Aliases, each counted once per domain. One naming the domain's own
  // local or canonical name finds that spelling taken already, so it needs no
  // rule of its own here.
  const listed = new Map<string, string[]>();
  for (const row of rows) {
    for (const alias of new Set(row.aliases)) {
      listed.set(alias, [...(listed.get(alias) ?? []), row.name]);
    }
  }
  const granted: [string, string][] = [];
  for (const [alias, owners] of listed) {
    const [only] = owners;
    if (
      only !== undefined &&
      owners.length === 1 &&
      !spellings.has(alias) &&
      !contested.has(alias)
    ) {
      granted.push([alias, only]);
    }
  }
  for (const [alias, local] of granted) {
    spellings.set(alias, local);
  }
  return spellings;
}
