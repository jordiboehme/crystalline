/**
 * The payload, as the picture the renderer is handed.
 *
 * This is the whole of what the graph view claims, testable without a canvas:
 * which engrams are drawn, how they are labelled, which of them are marked
 * retired, and which arrows survive. Two of these are ways the view could break
 * rather than merely look wrong - an arrow with an end the payload never named
 * makes the renderer throw, and so does a repeated id - so both are pinned here
 * instead of discovered as a blank panel.
 */

import { describe, expect, it } from "vitest";

import { readGraph } from "./api/graph";
import type { GraphElement, GraphNodeElement } from "./graphElements";
import {
  ANCHOR_CLASS,
  FADED_CLASS,
  graphConnections,
  graphElements,
  isEdgeElement,
} from "./graphElements";

/** Alpha is the anchor, Beta is retired, Gamma is live. */
function neighborhood(overrides: Record<string, unknown> = {}) {
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
        status: "deprecated",
        type: "decision",
      },
      {
        id: 3,
        domain: "ops",
        permalink: "gamma",
        title: "Gamma",
        status: "stable",
        type: "runbook",
      },
    ],
    edges: [
      { from: 1, to: 2, rel_type: "supersedes" },
      { from: 3, to: 1, rel_type: "links_to" },
    ],
    truncated: false,
    ...overrides,
  });
}

const ANCHOR = { domain: "eng", permalink: "alpha" };

/** The engrams on the picture, which are the elements carrying an address. */
function nodes(elements: GraphElement[]): GraphNodeElement[] {
  return elements.filter(
    (element): element is GraphNodeElement => !isEdgeElement(element),
  );
}

/** The arrows between them. */
function edges(elements: GraphElement[]) {
  return elements.filter(isEdgeElement);
}

describe("the graph elements", () => {
  it("draws every engram in the payload and every arrow between them", () => {
    const elements = graphElements(neighborhood(), ANCHOR);

    expect(nodes(elements)).toHaveLength(3);
    expect(edges(elements)).toHaveLength(2);
    expect(nodes(elements).map((node) => node.data.label)).toEqual([
      "Alpha",
      "Beta",
      "Gamma",
    ]);
  });

  it("labels an arrow with the relation it is", () => {
    const elements = graphElements(neighborhood(), ANCHOR);

    expect(edges(elements).map((edge) => edge.data.label)).toEqual([
      "supersedes",
      "links_to",
    ]);
  });

  it("says nothing rather than inventing a relation the payload left out", () => {
    const elements = graphElements(
      neighborhood({ edges: [{ from: 1, to: 2 }] }),
      ANCHOR,
    );

    expect(edges(elements)[0].data.label).toBe("");
  });

  it("fades a retired engram and never leaves it out", () => {
    const elements = graphElements(neighborhood(), ANCHOR);
    const [alpha, beta, gamma] = nodes(elements);

    // Retired is part of what the domain holds: it is drawn, and drawn faded.
    expect(beta.classes).toContain(FADED_CLASS);
    expect(alpha.classes).not.toContain(FADED_CLASS);
    expect(gamma.classes).not.toContain(FADED_CLASS);
  });

  it("marks the engram the neighborhood was drawn around", () => {
    const elements = graphElements(neighborhood(), ANCHOR);
    const [alpha, beta] = nodes(elements);

    expect(alpha.classes).toContain(ANCHOR_CLASS);
    expect(beta.classes).not.toContain(ANCHOR_CLASS);
  });

  it("carries each engram's address, which is what a click needs", () => {
    const elements = graphElements(neighborhood(), ANCHOR);

    expect(nodes(elements)[2].data).toMatchObject({
      domain: "ops",
      permalink: "gamma",
    });
  });

  it("drops an arrow with an end nothing in the payload named", () => {
    // The renderer throws on an edge into a node it has never seen, so a
    // payload the reader trimmed must not take the whole picture down.
    const elements = graphElements(
      neighborhood({
        edges: [
          { from: 1, to: 2, rel_type: "supersedes" },
          { from: 1, to: 99, rel_type: "links_to" },
        ],
      }),
      ANCHOR,
    );

    expect(edges(elements)).toHaveLength(1);
    expect(edges(elements)[0].data.label).toBe("supersedes");
  });

  it("draws one element per id however often the payload repeats it", () => {
    // A repeated id is the other way the renderer throws.
    const elements = graphElements(
      neighborhood({
        edges: [
          { from: 1, to: 2, rel_type: "supersedes" },
          { from: 1, to: 2, rel_type: "supersedes" },
        ],
      }),
      ANCHOR,
    );

    expect(edges(elements)).toHaveLength(1);
    const ids = elements.map((element) => element.data.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("draws nothing at all from an empty neighborhood", () => {
    expect(
      graphElements({ nodes: [], edges: [], truncated: false }, ANCHOR),
    ).toEqual([]);
  });
});

/**
 * The same neighborhood as sentences, which is what anything that cannot see a
 * canvas is given. It has to carry what the picture carries - both ends of each
 * arrow and the relation it is - or the reader is handed a bag of names.
 */
describe("the graph connections", () => {
  it("names both ends of each arrow and the relation it is", () => {
    const connections = graphConnections(neighborhood());

    expect(
      connections.map((one) => [one.from.title, one.relType, one.to.title]),
    ).toEqual([
      ["Alpha", "supersedes", "Beta"],
      ["Gamma", "links_to", "Alpha"],
    ]);
  });

  it("carries each end whole, status and address included", () => {
    const [first] = graphConnections(neighborhood());

    // The status is what fades a retired end, and the address is what the
    // link points at: both come from the node rather than from the edge.
    expect(first.to).toMatchObject({
      domain: "eng",
      permalink: "notes/beta",
      status: "deprecated",
    });
  });

  it("keeps an arrow between two engrams that are not the anchor", () => {
    // Which is the whole of what a second hop adds: at depth two the payload
    // carries edges neither end of which is the engram being read.
    const connections = graphConnections(
      neighborhood({ edges: [{ from: 2, to: 3, rel_type: "relates_to" }] }),
    );

    expect(connections).toHaveLength(1);
    expect(connections[0].from.title).toBe("Beta");
    expect(connections[0].to.title).toBe("Gamma");
  });

  it("agrees with the picture about which arrows there are", () => {
    // Same filtering, same dedupe, same ids: the text and the drawing are one
    // answer in two forms rather than two answers.
    const graph = neighborhood({
      edges: [
        { from: 1, to: 2, rel_type: "supersedes" },
        { from: 1, to: 2, rel_type: "supersedes" },
        { from: 1, to: 99, rel_type: "links_to" },
      ],
    });

    expect(graphConnections(graph).map((one) => one.id)).toEqual(
      edges(graphElements(graph, ANCHOR)).map((edge) => edge.data.id),
    );
  });

  it("says nothing about a relation the payload did not name", () => {
    const connections = graphConnections(
      neighborhood({ edges: [{ from: 1, to: 2 }] }),
    );

    expect(connections[0].relType).toBeNull();
  });
});
