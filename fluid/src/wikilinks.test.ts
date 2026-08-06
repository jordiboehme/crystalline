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
      ],
      relations: [],
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
    ],
    edges: [],
    truncated: false,
  });
}

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

  it("says nothing about bracket text the server never parsed as a reference", () => {
    const resolve = buildWikilinkResolver(detail(), graph());

    expect(resolve("Nowhere At All")).toBeNull();
  });
});
