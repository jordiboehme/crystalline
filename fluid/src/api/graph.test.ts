/**
 * Turning a flat edge list into the answer to "what points here".
 *
 * The graph payload is undirected in shape and directed in meaning: every edge
 * within one hop of the anchor is in it, pointing either way, and which half of
 * that is a backlink is this function's whole job. Getting the direction wrong
 * would put an engram's own outbound references in its backlinks panel, which
 * reads as a knowledge base far better connected than it is.
 */

import { describe, expect, it } from "vitest";

import { backlinksTo, readGraph } from "./graph";

/** Alpha is the anchor, Beta points at it, Gamma is pointed at by it. */
function neighborhood() {
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
        domain: "eng",
        permalink: "gamma",
        title: "Gamma",
        status: "stable",
        type: "engram",
      },
    ],
    edges: [
      { from: 2, to: 1, rel_type: "links_to" },
      { from: 2, to: 1, rel_type: "supersedes" },
      { from: 1, to: 3, rel_type: "links_to" },
      { from: 1, to: 1, rel_type: "links_to" },
    ],
    truncated: false,
  });
}

describe("backlinks", () => {
  it("keeps the edges pointing at the anchor and drops the ones leaving it", () => {
    const found = backlinksTo(neighborhood(), "eng", "alpha");

    expect(found.map((backlink) => backlink.node.permalink)).toEqual([
      "notes/beta",
    ]);
  });

  it("groups one source's several relation types into one entry", () => {
    const [beta] = backlinksTo(neighborhood(), "eng", "alpha");

    expect(beta.relTypes).toEqual(["links_to", "supersedes"]);
  });

  it("never counts an engram as pointing at itself", () => {
    const found = backlinksTo(neighborhood(), "eng", "alpha");

    expect(found.some((backlink) => backlink.node.permalink === "alpha")).toBe(
      false,
    );
  });

  it("answers nothing when the anchor is not in the payload", () => {
    // The node ids are opaque and local to one response, so an anchor that is
    // not there cannot be matched by id without matching some other engram by
    // accident.
    expect(backlinksTo(neighborhood(), "eng", "ghost")).toEqual([]);
    expect(backlinksTo(undefined, "eng", "alpha")).toEqual([]);
  });

  it("drops nodes and edges that carry no address", () => {
    const graph = readGraph({
      nodes: [{ id: 1, domain: "eng" }, null, "nonsense"],
      edges: [{ from: 1 }, { from: 1, to: 2, rel_type: "links_to" }],
    });

    expect(graph.nodes).toEqual([]);
    expect(graph.edges).toEqual([{ from: 1, to: 2, relType: "links_to" }]);
    expect(graph.truncated).toBe(false);
  });
});
