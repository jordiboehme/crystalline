/**
 * When `[[Target]]` becomes a link, and when it deliberately does not.
 *
 * Three states rather than two, and the middle one is the point. The detail
 * payload says whether the index resolved a reference; the graph says where it
 * landed. Between the two requests there is a moment when a link is known to
 * resolve and not yet known to resolve to what, and the honest rendering of
 * that moment is prose. Marking it broken there would be a claim the app
 * cannot back, and it would flicker into a link a moment later.
 */

import { describe, expect, it } from "vitest";

import { readEngramDetail } from "./api/engram";
import { readGraph } from "./api/graph";
import { domainSpellings } from "./domainNames";
import { buildWikilinkResolver, parseWikiTarget } from "./wikilinks";

function detail() {
  return readEngramDetail(
    {
      domain: "eng",
      permalink: "alpha",
      title: "Alpha",
      content: "",
      links: [
        { line: 1, resolved: true, target: { domain: null, target: "Beta" } },
        { line: 1, resolved: false, target: { domain: null, target: "Ghost" } },
        {
          line: 2,
          resolved: true,
          target: { domain: "ops", target: "Runbook" },
        },
        // An engram whose own title starts with a word and a colon. The server
        // parsed it as a prefix, because that is all the bracket text says, and
        // resolved it at home.
        {
          line: 3,
          resolved: true,
          target: { domain: "Log", target: "Weekly Garden Notes" },
        },
      ],
      relations: [
        // The shape the engine writes for itself: the successor named by its
        // permalink, because a permalink cannot be misread as a domain prefix.
        {
          line: 4,
          relType: "superseded_by",
          resolved: true,
          target: { domain: null, target: "notes/beta" },
        },
      ],
    },
    "eng",
    "alpha",
  );
}

function graph() {
  return readGraph({
    nodes: [
      {
        id: 1,
        domain: "eng",
        permalink: "alpha",
        title: "Alpha",
        status: "stable",
        type: "engram",
      },
      {
        id: 2,
        domain: "eng",
        permalink: "notes/beta",
        title: "Beta",
        status: "stable",
        type: "engram",
      },
      {
        id: 3,
        domain: "ops",
        permalink: "runbooks/restart",
        title: "Runbook",
        status: "stable",
        type: "runbook",
      },
      {
        id: 4,
        domain: "eng",
        permalink: "log-weekly",
        title: "Log: Weekly Garden Notes",
        status: "stable",
        type: "engram",
      },
    ],
    edges: [],
    truncated: false,
  });
}

/** The domain listing the app already holds, as the resolver takes it. */
const DOMAINS = domainSpellings([
  { name: "eng", canonicalName: "eng", aliases: [] },
  { name: "ops", canonicalName: "ops", aliases: [] },
]);

describe("parsing what is inside the brackets", () => {
  it("reads a leading domain prefix", () => {
    expect(parseWikiTarget("ops:Runbook")).toEqual({
      domain: "ops",
      target: "Runbook",
    });
  });

  it("leaves a colon that is punctuation in the target", () => {
    // A domain segment never has whitespace in it, so this is a title with a
    // colon rather than a cross-domain reference to a domain called "Note".
    expect(parseWikiTarget("Note on things: the sequel")).toEqual({
      domain: null,
      target: "Note on things: the sequel",
    });
  });

  it("cannot tell a one-word title prefix from a domain, and does not try", () => {
    // The half of the same case that does have whitespace-free text before the
    // colon. This split is wrong for `Log: Weekly Garden Notes` and right for
    // `ops:Runbook`, and nothing inside the brackets says which is which - so
    // the parser stays the server's mirror and the resolver settles it against
    // the domain list.
    expect(parseWikiTarget("Log: Weekly Garden Notes")).toEqual({
      domain: "Log",
      target: "Weekly Garden Notes",
    });
  });
});

describe("the wikilink resolver", () => {
  it("points a resolved target at the address the graph gives it", () => {
    const resolve = buildWikilinkResolver(detail(), graph());

    expect(resolve("Beta")).toEqual({
      kind: "resolved",
      href: "/d/eng/e/notes/beta",
      label: "Beta",
    });
  });

  it("follows a cross-domain reference into the other domain", () => {
    const resolve = buildWikilinkResolver(detail(), graph());

    expect(resolve("ops:Runbook")).toEqual({
      kind: "resolved",
      href: "/d/ops/e/runbooks/restart",
      label: "Runbook",
    });
  });

  it("marks a target the index looked for and did not find", () => {
    const resolve = buildWikilinkResolver(detail(), graph());

    expect(resolve("Ghost")).toEqual({ kind: "unresolved" });
  });

  it("says nothing about a resolved target while the graph is still coming", () => {
    const resolve = buildWikilinkResolver(detail(), undefined);

    expect(resolve("Beta")).toBeNull();
    // The negative is known from the detail payload alone, so it is drawn
    // straight away rather than waiting on a request that cannot change it.
    expect(resolve("Ghost")).toEqual({ kind: "unresolved" });
  });

  it("labels a link written by permalink with the engram's title", () => {
    const resolve = buildWikilinkResolver(detail(), graph(), DOMAINS);

    // The file carries the address and a reader keeps seeing the name. This is
    // what lets the engine write the stable identity into a relation without
    // costing anybody the title.
    expect(resolve("notes/beta")).toEqual({
      kind: "resolved",
      href: "/d/eng/e/notes/beta",
      label: "Beta",
    });
  });

  it("says nothing about bracket text the server never parsed as a reference", () => {
    const resolve = buildWikilinkResolver(detail(), graph());

    expect(resolve("Nowhere At All")).toBeNull();
  });

  it("reads a prefix no domain answers to as part of the title", () => {
    // Nothing is registered under `Log`, so the whole bracket text is a title
    // in this engram's own domain - which is where the graph put the engram
    // and the only key it can be found under.
    const resolve = buildWikilinkResolver(detail(), graph(), DOMAINS);

    expect(resolve("Log: Weekly Garden Notes")).toEqual({
      kind: "resolved",
      href: "/d/eng/e/log-weekly",
      label: "Log: Weekly Garden Notes",
    });
  });

  it("keeps a prefix that names a real domain a prefix", () => {
    const resolve = buildWikilinkResolver(detail(), graph(), DOMAINS);

    expect(resolve("ops:Runbook")).toEqual({
      kind: "resolved",
      href: "/d/ops/e/runbooks/restart",
      label: "Runbook",
    });
  });

  it("asks nothing of a caller that has no domain listing to give", () => {
    // No list is not an empty list: a caller that cannot tell gets the reading
    // the parser gave and no second guess, which leaves this one pending.
    const resolve = buildWikilinkResolver(detail(), graph());

    expect(resolve("Log: Weekly Garden Notes")).toBeNull();
  });
});

/**
 * A domain registered here as `moonbase` whose MANIFEST calls it `moon` and
 * which used to be called `lunar`; a domain `sky` beside one registered as
 * `skybase` that also calls itself `sky`, so `sky` stays the first one's, and
 * lists `moonbase` as a former name, which the local name keeps. Links carry
 * the domain as it was written, and the server binds every spelling.
 */
const NAMED = domainSpellings([
  { name: "eng", canonicalName: "eng", aliases: [] },
  {
    name: "moonbase",
    canonicalName: "moon",
    aliases: ["lunar"],
  },
  { name: "sky", canonicalName: "sky", aliases: [] },
  {
    name: "skybase",
    canonicalName: "sky",
    aliases: ["moonbase"],
  },
]);

function resolvedLink(line: number, domain: string | null, target: string) {
  return { line, resolved: true, target: { domain, target } };
}

function namedDetail(home: string, links: ReturnType<typeof resolvedLink>[]) {
  return readEngramDetail(
    { domain: home, permalink: "here", title: "Here", content: "", links },
    home,
    "here",
  );
}

function namedGraph() {
  const node = (
    id: number,
    domain: string,
    permalink: string,
    title: string,
  ) => ({
    id,
    domain,
    permalink,
    title,
    status: "stable",
    type: "engram",
  });
  return readGraph({
    nodes: [
      node(1, "moonbase", "crater-base", "Crater Base"),
      node(2, "sky", "star", "Star"),
      node(3, "skybase", "star", "Star"),
    ],
    edges: [],
    truncated: false,
  });
}

const CRATER = {
  kind: "resolved",
  href: "/d/moonbase/e/crater-base",
  label: "Crater Base",
};

describe("a link that names its domain by another name", () => {
  it("follows a canonical name to the domain that declares it", () => {
    const detail = namedDetail("eng", [resolvedLink(1, "moon", "Crater Base")]);
    const resolve = buildWikilinkResolver(detail, namedGraph(), NAMED);

    expect(resolve("moon:Crater Base")).toEqual(CRATER);
  });

  it("follows an alias to the domain that answers to it", () => {
    const detail = namedDetail("eng", [
      resolvedLink(1, "lunar", "Crater Base"),
    ]);
    const resolve = buildWikilinkResolver(detail, namedGraph(), NAMED);

    expect(resolve("lunar:Crater Base")).toEqual(CRATER);
  });

  it("sends a shadowed canonical name to the domain registered under it", () => {
    const detail = namedDetail("eng", [resolvedLink(1, "sky", "Star")]);
    const resolve = buildWikilinkResolver(detail, namedGraph(), NAMED);

    expect(resolve("sky:Star")).toEqual({
      kind: "resolved",
      href: "/d/sky/e/star",
      label: "Star",
    });
  });

  it("still follows the local name", () => {
    const detail = namedDetail("eng", [
      resolvedLink(1, "moonbase", "Crater Base"),
    ]);
    const resolve = buildWikilinkResolver(detail, namedGraph(), NAMED);

    expect(resolve("moonbase:Crater Base")).toEqual(CRATER);
  });

  it("leaves a name no domain answers to as prose", () => {
    // The `resolved: true` is the index's verdict: its resolve pass
    // (`reference_match` in `index/src/store.rs`) falls back to the home
    // domain for a prefix no spelling names, so it binds this link to home's
    // `Crater Base`. `core/src/address.rs` instead reads the whole bracket
    // text as a title at home. The two disagree, and this pins what Fluid does
    // until that is settled: it follows core, and the link stays prose.
    const detail = namedDetail("moonbase", [
      resolvedLink(1, "mars", "Crater Base"),
    ]);
    const resolve = buildWikilinkResolver(detail, namedGraph(), NAMED);

    expect(resolve("mars:Crater Base")).toBeNull();
  });

  it("treats the home domain's canonical name as the home domain", () => {
    const detail = namedDetail("moonbase", [
      resolvedLink(1, "moon", "Crater Base"),
      resolvedLink(2, null, "Crater Base"),
    ]);
    const resolve = buildWikilinkResolver(detail, namedGraph(), NAMED);

    expect(resolve("moon:Crater Base")).toEqual(CRATER);
    expect(resolve("Crater Base")).toEqual(CRATER);
  });

  it("takes the verdict of a reference written with another spelling", () => {
    // The relation names the local name, the prose the canonical one: the
    // server reports them as one target, and so does the resolver.
    const detail = readEngramDetail(
      {
        domain: "eng",
        permalink: "here",
        title: "Here",
        content: "",
        relations: [
          {
            relType: "uses",
            ...resolvedLink(1, "moonbase", "Crater Base"),
          },
        ],
      },
      "eng",
      "here",
    );
    const resolve = buildWikilinkResolver(detail, namedGraph(), NAMED);

    expect(resolve("moon:Crater Base")).toEqual(CRATER);
  });

  it("matches the domain by its exact spelling", () => {
    // The server binds no spelling in another case, so these fall back to a
    // title in the engram's own domain, where nothing of that name lives.
    const detail = namedDetail("eng", [
      resolvedLink(1, "Moon", "Crater Base"),
      resolvedLink(2, "MoonBase", "Crater Base"),
    ]);
    const resolve = buildWikilinkResolver(detail, namedGraph(), NAMED);

    expect(resolve("Moon:Crater Base")).toBeNull();
    expect(resolve("MoonBase:Crater Base")).toBeNull();
  });
});
