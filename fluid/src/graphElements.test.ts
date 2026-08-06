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
