/**
 * The neighborhood around one engram, as `GET /graph` answers it: a flat node
 * list and a flat list of typed, directed edges between them.
 *
 * The engram page reads it for two things the detail payload cannot give it.
 * Backlinks, because the detail payload samples its inbound references and caps
 * the sample at five, while an edge list at one hop is the whole set within
 * that hop; and the addresses its outbound wikilinks resolve to, because the
 * detail payload reports a target as it was written rather than as a permalink.
 *
 * `id` is opaque and stable only within one response, so nothing outside a
 * single payload is ever keyed by it.
 */

import { api } from "./client";
import { crystallineAddress } from "./engram";
import { asArray, asNumber, asObject, asString } from "./json";

/** One engram in the neighborhood. */
export interface GraphNode {
  /** Opaque within this response, and meaningless outside it. */
  id: number;
  domain: string;
  permalink: string;
  /** Its title, falling back to the permalink when it has none. */
  title: string;
  /** Its `status` frontmatter, free form, or null when it carries none. */
  status: string | null;
  /** Its `type` frontmatter, free form, or null when it carries none. */
  type: string | null;
}

/** One directed reference between two nodes of this response. */
export interface GraphEdge {
  from: number;
  to: number;
  /** The relation type, `links_to` for a prose wikilink. */
  relType: string | null;
}

/** One `GET /graph` answer. */
export interface GraphNeighborhood {
  nodes: GraphNode[];
  edges: GraphEdge[];
  /** Whether the node cap cut anything out of this answer. */
  truncated: boolean;
}

/** An engram that points at the one being read, and how it points. */
export interface Backlink {
  node: GraphNode;
  /** Every relation type it points with, in the order the payload gave them. */
  relTypes: string[];
}

/** How many hops out an engram page opens on. A second hop is a deliberate act. */
export const NEIGHBORHOOD_DEPTH = 1;

/** Read one node, or null when it carries no address. */
function readNode(value: unknown): GraphNode | null {
  const record = asObject(value);
  const id = asNumber(record?.id);
  const domain = asString(record?.domain);
  const permalink = asString(record?.permalink);
  if (id === null || domain === null || permalink === null) {
    return null;
  }
  return {
    id,
    domain,
    permalink,
    title: asString(record?.title) ?? permalink,
    status: asString(record?.status),
    type: asString(record?.type),
  };
}

/** Read one edge, or null when either end is missing. */
function readEdge(value: unknown): GraphEdge | null {
  const record = asObject(value);
  const from = asNumber(record?.from);
  const to = asNumber(record?.to);
  if (from === null || to === null) {
    return null;
  }
  return { from, to, relType: asString(record?.rel_type) };
}

/** Read a graph payload. */
export function readGraph(payload: unknown): GraphNeighborhood {
  const record = asObject(payload);
  return {
    nodes: asArray(record?.nodes)
      .map(readNode)
      .filter((node): node is GraphNode => node !== null),
    edges: asArray(record?.edges)
      .map(readEdge)
      .filter((edge): edge is GraphEdge => edge !== null),
    truncated: record?.truncated === true,
  };
}

/**
 * The engrams pointing at this one, with the relation types they point with.
 *
 * Grouped by source rather than listed per edge: an engram that both declares
 * `- supersedes [[X]]` and writes the wikilink in its prose points at X once,
 * from a reader's point of view, with two things to say about how.
 *
 * The anchor is found by address rather than assumed to be first in the node
 * list, and an anchor that is not in the payload yields nothing: an unknown
 * anchor would otherwise match edge ends by accident.
 */
export function backlinksTo(
  graph: GraphNeighborhood | undefined,
  domain: string,
  permalink: string,
): Backlink[] {
  const anchor = graph?.nodes.find(
    (node) => node.domain === domain && node.permalink === permalink,
  );
  if (!graph || !anchor) {
    return [];
  }
  const byId = new Map(graph.nodes.map((node) => [node.id, node]));
  const found = new Map<number, Backlink>();
  for (const edge of graph.edges) {
    // A self-reference is not a backlink: an engram is not something that
    // points here from elsewhere.
    if (edge.to !== anchor.id || edge.from === anchor.id) {
      continue;
    }
    const node = byId.get(edge.from);
    if (!node) {
      continue;
    }
    const existing = found.get(node.id);
    if (!existing) {
      found.set(node.id, {
        node,
        relTypes: edge.relType === null ? [] : [edge.relType],
      });
    } else if (
      edge.relType !== null &&
      !existing.relTypes.includes(edge.relType)
    ) {
      existing.relTypes.push(edge.relType);
    }
  }
  return [...found.values()];
}

/** The cache key of one engram's neighborhood. */
export function graphKey(
  domain: string,
  permalink: string,
  depth: number,
): readonly unknown[] {
  return ["graph", domain, permalink, depth];
}

/** Fetch the neighborhood around one engram. */
export async function fetchGraph(
  domain: string,
  permalink: string,
  depth: number = NEIGHBORHOOD_DEPTH,
): Promise<GraphNeighborhood> {
  const query = new URLSearchParams({
    anchor: crystallineAddress(domain, permalink),
    depth: String(depth),
  });
  return readGraph(await api<unknown>(`/graph?${query.toString()}`));
}
