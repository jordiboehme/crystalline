/**
 * The one thing about the drawing a test without a canvas can still hold.
 *
 * The renderer runs headless as well as on a page, so the real stylesheet, the
 * real layout and real elements are put through it here. What that catches is
 * the failure mode this file exists for: a mistyped style property or a layout
 * name the library does not have are a warning on the console and a picture
 * that quietly comes out wrong, never an error anybody sees.
 *
 * The rest of the drawing - where a node lands, how it is painted - is the
 * library's business and not ours to assert.
 */

import cytoscape from "cytoscape";
import { describe, expect, it, vi } from "vitest";

import { readGraph } from "./api/graph";
import { graphElements } from "./graphElements";
import { GRAPH_LAYOUT, graphStylesheet } from "./graphStyle";

/** Alpha is the anchor and Beta, which it points at, is retired. */
function elements() {
  return graphElements(
    readGraph({
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
      ],
      edges: [{ from: 1, to: 2, rel_type: "links_to" }],
      truncated: false,
    }),
    { domain: "eng", permalink: "alpha" },
  );
}

describe("the graph stylesheet", () => {
  it.each([
    ["light", false],
    ["dark", true],
  ])("draws a %s neighborhood the renderer understands", (_name, dark) => {
    // Cytoscape reports a style property it does not know, and a layout it
    // cannot find, by warning and carrying on.
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);

    const cy = cytoscape({
      headless: true,
      styleEnabled: true,
      elements: elements(),
      style: graphStylesheet(dark),
      layout: GRAPH_LAYOUT,
    });

    expect(warn).not.toHaveBeenCalled();
    // The classes the mapping assigns are the ones the stylesheet selects on,
    // which is the join between the two halves of this feature.
    expect(cy.nodes(".anchor")).toHaveLength(1);
    expect(cy.nodes(".retired").style("opacity")).toBe("0.5");
    // And the arrow says the relation it is, read off the element's own data.
    expect(cy.edges().style("label")).toBe("links_to");
    cy.destroy();
    warn.mockRestore();
  });
});
