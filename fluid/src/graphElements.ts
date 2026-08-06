/**
 * The neighborhood payload, turned into the elements a graph renderer draws.
 *
 * Pure, and kept apart from the renderer on purpose: this is everything the
 * picture claims about the knowledge base - which engrams are on it, what each
 * arrow says, and which nodes are retired - so it can be pinned by tests with
 * no canvas to draw on, while the module that owns the drawing stays a thin
 * lazy chunk with nothing to assert about.
 *
 * Retired engrams are marked rather than dropped, the way every list in this
 * app fades them: a picture with the retired nodes cut out of it would show a
 * knowledge base tidier and better connected than the one on disk.
 *
 * Two shapes the renderer refuses are filtered out here rather than left to
 * throw: an arrow whose end is not among the nodes, and a repeated id. Neither
 * should arrive - the server drops an edge that lost an end to the node cap,
 * and dedupes what it sends - but the reader also drops nodes that carry no
 * address, and a renderer that threw would take the whole screen with it.
 */

import type { ElementDefinition } from "cytoscape";

import type { GraphNeighborhood } from "./api/graph";
import { isRetired } from "./lifecycle";

/** The class a retired engram carries, which the stylesheet fades. */
export const FADED_CLASS = "retired";

/** The class the engram the neighborhood was drawn around carries. */
export const ANCHOR_CLASS = "anchor";

/** Which engram a neighborhood was drawn around. */
export interface GraphAnchor {
  domain: string;
  permalink: string;
}

/** What a drawn engram carries: its label, and the address a click follows. */
export interface GraphNodeData {
  id: string;
  label: string;
  domain: string;
  permalink: string;
}

/** What a drawn arrow carries: its ends, and the relation it is. */
export interface GraphEdgeData {
  id: string;
  label: string;
  source: string;
  target: string;
}

/** One engram on the picture, with the classes that style it. */
export interface GraphNodeElement extends ElementDefinition {
  data: GraphNodeData;
  classes: string[];
}

/** One arrow on it. */
export interface GraphEdgeElement extends ElementDefinition {
  data: GraphEdgeData;
  classes: string[];
}

/**
 * One element of the picture, typed rather than left to the renderer's own
 * index-signature shape, so what a node and an arrow carry is stated once and
 * checked everywhere either is read.
 */
export type GraphElement = GraphNodeElement | GraphEdgeElement;

/** Whether this element is an arrow rather than an engram. */
export function isEdgeElement(
  element: GraphElement,
): element is GraphEdgeElement {
  return "source" in element.data;
}

/**
 * The elements for one payload: a node per engram, an edge per relation.
 *
 * Ids are the payload's own with the kind in front of them, because nodes and
 * edges share one id namespace in the renderer, and the payload's ids are only
 * opaque within one response anyway.
 */
export function graphElements(
  graph: GraphNeighborhood,
  anchor: GraphAnchor,
): GraphElement[] {
  const drawn = new Set<string>();
  const elements: GraphElement[] = [];

  for (const node of graph.nodes) {
    const id = nodeId(node.id);
    if (drawn.has(id)) {
      continue;
    }
    drawn.add(id);
    const classes: string[] = [];
    if (node.domain === anchor.domain && node.permalink === anchor.permalink) {
      classes.push(ANCHOR_CLASS);
    }
    if (isRetired(node.status)) {
      classes.push(FADED_CLASS);
    }
    elements.push({
      data: {
        id,
        label: node.title,
        domain: node.domain,
        permalink: node.permalink,
      },
      classes,
    });
  }

  for (const edge of graph.edges) {
    const source = nodeId(edge.from);
    const target = nodeId(edge.to);
    // An arrow into an engram nobody drew points at nothing.
    if (!drawn.has(source) || !drawn.has(target)) {
      continue;
    }
    // Written as it was declared, and left blank rather than named something
    // the payload never said.
    const label = edge.relType ?? "";
    const id = `edge-${String(edge.from)}-${String(edge.to)}-${label}`;
    if (drawn.has(id)) {
      continue;
    }
    drawn.add(id);
    elements.push({ data: { id, label, source, target }, classes: [] });
  }

  return elements;
}

/** The renderer's id for one engram. */
function nodeId(id: number): string {
  return `node-${String(id)}`;
}
