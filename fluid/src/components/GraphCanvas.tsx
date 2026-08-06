/**
 * The drawing itself: elements in, a picture on a canvas out.
 *
 * Its own module because of its own chunk. The graph library is heavy, most
 * visits never open a graph, and nothing else imports this file, so the bundler
 * gives it a chunk that arrives when somebody asks for a picture and never
 * otherwise. Everything the picture claims is decided before it gets here, in
 * `graphElements.ts`, and how it looks in `graphStyle.ts`, which is what leaves
 * this file with nothing a canvas-less test environment could see.
 *
 * The instance is rebuilt when the elements or the theme change and destroyed
 * on the way out: the library holds a canvas, a renderer and its own event
 * listeners, none of which React knows how to reclaim.
 */

import cytoscape from "cytoscape";
import type { EventObjectNode } from "cytoscape";
import { useEffect, useRef } from "react";

import type { GraphElement, GraphNodeData } from "../graphElements";
import { GRAPH_LAYOUT, graphStylesheet } from "../graphStyle";
import { useTheme } from "../theme/context";

export interface GraphCanvasProps {
  /** What to draw, already mapped. */
  elements: GraphElement[];
  /** Where a tap on an engram leads. */
  onSelect: (domain: string, permalink: string) => void;
}

export default function GraphCanvas({ elements, onSelect }: GraphCanvasProps) {
  const container = useRef<HTMLDivElement>(null);
  const { resolved } = useTheme();
  // Held in a ref so a caller that hands over a fresh callback does not cost a
  // rebuilt graph, and with it the layout the reader was looking at.
  const select = useRef(onSelect);
  useEffect(() => {
    select.current = onSelect;
  }, [onSelect]);

  useEffect(() => {
    const element = container.current;
    if (!element) {
      return;
    }
    const cy = cytoscape({
      container: element,
      elements,
      style: graphStylesheet(resolved === "dark"),
      layout: GRAPH_LAYOUT,
      // A neighborhood is for following, not for editing: dragging a box
      // around several nodes selects things nothing here acts on.
      boxSelectionEnabled: false,
      autounselectify: true,
    });
    cy.on("tap", "node", (event: EventObjectNode) => {
      const data = event.target.data() as GraphNodeData;
      select.current(data.domain, data.permalink);
    });
    return () => {
      cy.destroy();
    };
  }, [elements, resolved]);

  // The picture is decoration to anything that cannot see it: the same
  // neighborhood is listed as links beside it, which is what a screen reader
  // and a keyboard get to use. The library adds no focusable element of its
  // own, so nothing is being hidden away from a keyboard here.
  return <div ref={container} aria-hidden="true" className="h-full w-full" />;
}
