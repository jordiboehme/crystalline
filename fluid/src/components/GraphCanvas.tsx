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
 * The instance is rebuilt when the elements change and destroyed on the way
 * out: the library holds a canvas, a renderer and its own event listeners, none
 * of which React knows how to reclaim. A change of theme is not a rebuild - the
 * stylesheet is swapped on the instance that is already there, so switching to
 * dark repaints the picture instead of laying it out again under the reader.
 */

import cytoscape from "cytoscape";
import type { Core, EventObjectNode } from "cytoscape";
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
  const dark = resolved === "dark";
  // Held in refs so neither a caller handing over a fresh callback nor a change
  // of theme costs a rebuilt graph, and with it the layout the reader was
  // looking at.
  const select = useRef(onSelect);
  const instance = useRef<Core | null>(null);
  const isDark = useRef(dark);
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
      style: graphStylesheet(isDark.current),
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
    instance.current = cy;
    return () => {
      instance.current = null;
      cy.destroy();
    };
  }, [elements]);

  // A repaint rather than a rebuild: the elements and their positions are what
  // they were, and only the palette moved.
  useEffect(() => {
    isDark.current = dark;
    instance.current?.style(graphStylesheet(dark));
  }, [dark]);

  // The picture is decoration to anything that cannot see it: the same
  // neighborhood is listed as links beside it, which is what a screen reader
  // and a keyboard get to use. The library adds no focusable element of its
  // own, so nothing is being hidden away from a keyboard here.
  return <div ref={container} aria-hidden="true" className="h-full w-full" />;
}
