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
import type { StylesheetJson } from "cytoscape";
import { describe, expect, it, vi } from "vitest";

import { readGraph } from "./api/graph";
import { ANCHOR_CLASS, graphElements } from "./graphElements";
import { GRAPH_LAYOUT, HOVERED_CLASS, graphStylesheet } from "./graphStyle";

/** What one selector in the sheet declares, read back by name. */
function styleFor(sheet: StylesheetJson, selector: string) {
  const block = sheet.find(
    (entry) => "selector" in entry && entry.selector === selector,
  );
  if (!block || !("style" in block)) {
    throw new Error(`no block for '${selector}' in the stylesheet`);
  }
  return block.style as Record<string, unknown>;
}

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
    // An arrow says nothing until it is pointed at, and then it says the
    // relation it is, read off the element's own data.
    expect(cy.edges().style("label")).toBe("");
    cy.edges().addClass(HOVERED_CLASS);
    expect(cy.edges().style("label")).toBe("links_to");
    cy.destroy();
    warn.mockRestore();
  });

  it("keeps node labels UI-sized and haloed, and edges silent until hovered", () => {
    // The picture is a small map, not a poster: the labels are the size of the
    // app's own small text and are cut out of the canvas behind them, so a name
    // crossing an arrow stays readable without the arrow shouting back.
    const sheet = graphStylesheet(false);
    const node = styleFor(sheet, "node");
    expect(node["font-size"]).toBeLessThanOrEqual(9);
    expect(node["text-outline-width"]).toBeGreaterThanOrEqual(2);

    const edge = styleFor(sheet, "edge");
    expect(edge.label).toBe("");
    const hovered = styleFor(sheet, `edge.${HOVERED_CLASS}`);
    expect(hovered.label).toBe("data(label)");
    expect(hovered["text-rotation"]).toBe("none");
  });

  it("gives the anchor the app's own accent in both themes", () => {
    // The one thing on the picture that is not neutral geometry is the engram
    // the reader is standing on, and it wears the accent every other screen
    // uses for the same job.
    expect(
      styleFor(graphStylesheet(false), `node.${ANCHOR_CLASS}`)[
        "background-color"
      ],
    ).toBe("#0f766e");
    expect(
      styleFor(graphStylesheet(true), `node.${ANCHOR_CLASS}`)[
        "background-color"
      ],
    ).toBe("#2dd4bf");
  });

  it("repaints an instance that is already drawn, without moving it", () => {
    // What lets a change of theme swap the stylesheet on the graph the reader
    // is looking at instead of building a new one and laying it out again.
    const cy = cytoscape({
      headless: true,
      styleEnabled: true,
      elements: elements(),
      style: graphStylesheet(false),
      layout: GRAPH_LAYOUT,
    });
    const light = String(cy.edges().style("line-color"));
    const where = { ...cy.nodes().first().position() };

    cy.style(graphStylesheet(true));

    expect(String(cy.edges().style("line-color"))).not.toBe(light);
    expect(cy.nodes().first().position()).toEqual(where);
    cy.destroy();
  });
});
