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

/**
 * The class an arrow wears while the pointer is on it, which is the only time
 * it says what relation it is.
 */
export const HOVERED_CLASS = "hovered";

/**
 * The stylesheet, for a dark page or a light one.
 *
 * Restraint is the whole design here. A neighborhood of three engrams is fitted
 * to the frame and comes out enormous, so every label was reading as a headline
 * while the arrows underneath carried a second layer of type at every angle -
 * a picture that shouted a handful of facts. So: node names at the app's own
 * small size, cut out of the surface behind them with an outline so a name
 * crossing an arrow stays legible, one accent for the engram the reader is
 * standing on, and arrows that are silent geometry until the pointer names one.
 * The zoom cap that keeps the fitted picture from magnifying all of this lives
 * with the instance, in `GraphCanvas.tsx`.
 *
 * `surface` has to be the color the canvas actually sits on, since the label
 * outline is a fake cut-out rather than real transparency: it is the container's
 * own background in `NeighborhoodGraph.tsx` - white, and slate-900 in the dark.
 */
export function graphStylesheet(dark: boolean): StylesheetJson {
  const ink = dark ? "#e2e8f0" : "#0f172a";
  const muted = dark ? "#94a3b8" : "#475569";
  const line = dark ? "#475569" : "#94a3b8";
  const surface = dark ? "#0f172a" : "#ffffff";
  return [
    {
      selector: "node",
      style: {
        "background-color": dark ? "#64748b" : "#94a3b8",
        label: "data(label)",
        color: ink,
        "font-size": 11,
        "text-valign": "bottom",
        "text-margin-y": 5,
        "text-wrap": "ellipsis",
        "text-max-width": "140px",
        "text-outline-color": surface,
        "text-outline-width": 2,
        "text-outline-opacity": 0.9,
        width: 14,
        height: 14,
      },
    },
    {
      // The engram the neighborhood was drawn around, so the reader can tell
      // where they are standing: the one thing on the picture that is not
      // neutral geometry, in the accent every other screen uses to say "here".
      selector: `node.${ANCHOR_CLASS}`,
      style: {
        "background-color": dark ? "#2dd4bf" : "#0f766e",
        width: 20,
        height: 20,
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
        "arrow-scale": 0.7,
        "curve-style": "bezier",
        label: "",
      },
    },
    {
      // What an arrow is, on demand. Horizontal rather than following the line,
      // because a label that rotates with its arrow is read at whatever angle
      // the layout happened to leave it at. Anything that cannot hover reads
      // the same arrows written out under the picture.
      selector: `edge.${HOVERED_CLASS}`,
      style: {
        label: "data(label)",
        color: muted,
        "font-size": 10,
        "text-outline-color": surface,
        "text-outline-width": 2,
        "text-rotation": "none",
      },
    },
  ];
}
