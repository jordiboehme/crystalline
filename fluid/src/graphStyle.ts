/**
 * How the neighborhood is drawn: the palette, the shapes and the layout.
 *
 * Apart from the component that mounts the renderer, so it can be run against
 * the real library with no canvas under it: the library reports an unknown
 * style property or a layout it cannot find by warning and carrying on, which
 * on a page is a picture that quietly comes out wrong. Here it is a test.
 *
 * Two palettes, following the app's own theme, because a canvas draws its own
 * colors and would otherwise stay stubbornly light behind a dark page.
 */

import type { LayoutOptions, StylesheetJson } from "cytoscape";

import { ANCHOR_CLASS, FADED_CLASS } from "./graphElements";

/**
 * Force directed, and settled before it is shown rather than animated into
 * place: the picture is there to be read, not watched.
 */
export const GRAPH_LAYOUT: LayoutOptions = {
  name: "cose",
  animate: false,
  padding: 24,
};

/** The stylesheet, for a dark page or a light one. */
export function graphStylesheet(dark: boolean): StylesheetJson {
  const ink = dark ? "#e2e8f0" : "#0f172a";
  const muted = dark ? "#94a3b8" : "#64748b";
  const line = dark ? "#475569" : "#cbd5e1";
  return [
    {
      selector: "node",
      style: {
        "background-color": dark ? "#64748b" : "#94a3b8",
        label: "data(label)",
        color: ink,
        "font-size": 10,
        "text-valign": "bottom",
        "text-margin-y": 4,
        "text-wrap": "ellipsis",
        "text-max-width": "120px",
        width: 18,
        height: 18,
      },
    },
    {
      // The engram the neighborhood was drawn around, so the reader can tell
      // where they are standing.
      selector: `node.${ANCHOR_CLASS}`,
      style: {
        "background-color": dark ? "#38bdf8" : "#0284c7",
        width: 26,
        height: 26,
        "font-weight": "bold",
      },
    },
    {
      // Retired, and on the picture: faded the way it is faded in every list.
      selector: `node.${FADED_CLASS}`,
      style: { opacity: 0.5 },
    },
    {
      selector: "edge",
      style: {
        width: 1,
        "line-color": line,
        "target-arrow-color": line,
        "target-arrow-shape": "triangle",
        "arrow-scale": 0.8,
        "curve-style": "bezier",
        label: "data(label)",
        color: muted,
        "font-size": 8,
        "text-rotation": "autorotate",
      },
    },
  ];
}
