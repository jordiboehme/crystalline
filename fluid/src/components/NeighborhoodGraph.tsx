/**
 * The neighborhood around one engram, drawn.
 *
 * A seam rather than the renderer. The graph library is the second heaviest
 * thing this app can load and most visits never open a graph, so it lives
 * behind a lazy import in `GraphCanvas.tsx` and arrives with the first picture
 * somebody asks for. Everything the picture claims is computed here instead, by
 * the pure mapping in `graphElements.ts`, which is what the tests can hold.
 *
 * It reads through `api/graph.ts` under that module's own cache key, which is
 * the key the engram page's backlinks panel already reads the same neighborhood
 * under: opening the graph there draws what the page has rather than waiting on
 * a payload of its own.
 *
 * Under the drawing the same neighborhood is written out, arrow by arrow: both
 * ends named and linked with the relation between them, and any engram no
 * surviving arrow mentions listed after. A canvas is one opaque element to a
 * screen reader and untabbable to a keyboard, so this is not a courtesy index
 * of names - it is the picture's own content in the form anything can read, and
 * it is derived from the same payload by the same rules so the two cannot drift
 * apart.
 */

import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Suspense, lazy, useCallback, useMemo } from "react";
import { Link, useNavigate } from "react-router";

import { ApiProblem, problemDetail } from "../api/client";
import { crystallineAddress } from "../api/engram";
import type { GraphNode } from "../api/graph";
import { fetchGraph, graphKey } from "../api/graph";
import { plural } from "../format";
import type { GraphAnchor } from "../graphElements";
import { graphConnections, graphElements } from "../graphElements";
import { RETIRED_CLASS, isRetired } from "../lifecycle";
import { engramRoute } from "../paths";

const GraphCanvas = lazy(() => import("./GraphCanvas"));

export interface NeighborhoodGraphProps {
  /** The engram the neighborhood is drawn around. */
  anchor: GraphAnchor;
  /** How many hops out, which the caller takes from the URL or fixes at one. */
  depth: number;
  /** How tall the drawing is, as a utility class. */
  height?: string;
}

export function NeighborhoodGraph({
  anchor,
  depth,
  height = "h-96",
}: NeighborhoodGraphProps) {
  const { domain, permalink } = anchor;
  const navigate = useNavigate();

  const graph = useQuery({
    queryKey: graphKey(domain, permalink, depth),
    queryFn: () => fetchGraph(domain, permalink, depth),
  });

  // Keyed by the address rather than by the anchor object, so a caller that
  // builds the prop inline does not redraw the graph on every render.
  const elements = useMemo(
    () => (graph.data ? graphElements(graph.data, { domain, permalink }) : []),
    [graph.data, domain, permalink],
  );

  const open = useCallback(
    (nodeDomain: string, nodePermalink: string) => {
      void navigate(engramRoute(nodeDomain, nodePermalink));
    },
    [navigate],
  );

  if (graph.isPending) {
    return <Quiet>Reading the neighborhood</Quiet>;
  }
  if (graph.error instanceof ApiProblem && graph.error.status === 404) {
    return (
      <Quiet>
        No engram at {crystallineAddress(domain, permalink)}, so there is no
        neighborhood to draw.
      </Quiet>
    );
  }
  if (graph.error) {
    return (
      <p
        role="alert"
        className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
      >
        {/*
          The framing says what the reader lost; the server's own detail says
          why, verbatim, the way every other error surface in this app shows
          it. On the full view this message is the whole screen, so a house
          sentence on its own would leave a reader with nothing to act on and
          nothing to report.
        */}
        The neighborhood could not be read, so there is nothing to draw:{" "}
        {problemDetail(graph.error)}
      </p>
    );
  }

  const neighborhood = graph.data;
  // The picture, said out loud. Derived from the same payload by the same
  // rules the drawing is, so the two are one answer in two forms.
  const connections = graphConnections(neighborhood);
  const named = new Set(
    connections.flatMap((connection) => [connection.from.id, connection.to.id]),
  );
  // An engram no surviving arrow mentions is still on the picture, so it is
  // still in the text: a list that quietly dropped it would be the shorter
  // kind of lie.
  const stranded = neighborhood.nodes.filter((node) => !named.has(node.id));

  // The anchor is in its own neighborhood, so one node is an engram on its own:
  // a single dot with no arrows is a picture that says less than a sentence.
  if (neighborhood.nodes.length <= 1) {
    return (
      <Quiet>
        Nothing is connected to this engram within {depth}{" "}
        {depth === 1 ? "hop" : "hops"} yet.
      </Quiet>
    );
  }

  return (
    <div className="flex flex-col gap-3">
      <div
        className={`${height} w-full overflow-hidden rounded border border-slate-200 dark:border-slate-800`}
      >
        <Suspense
          fallback={
            <p className="p-3 text-sm text-slate-500 dark:text-slate-400">
              Drawing the neighborhood
            </p>
          }
        >
          <GraphCanvas elements={elements} onSelect={open} />
        </Suspense>
      </div>

      {neighborhood.truncated && (
        <p className="text-xs text-slate-500 dark:text-slate-400">
          Showing the first {neighborhood.nodes.length} engrams: a neighborhood
          is capped at what one view can draw, so this is a bounded picture of
          it rather than the whole of it.
        </p>
      )}

      {neighborhood.hidden > 0 && (
        <p className="text-xs text-slate-500 dark:text-slate-400">
          {plural(
            neighborhood.hidden,
            "node beyond the cap is not drawn, retired ones first.",
            "nodes beyond the cap are not drawn, retired ones first.",
          )}
        </p>
      )}

      <ul
        aria-label="Connections in this neighborhood"
        className="flex flex-col gap-1 text-sm"
      >
        {connections.map((connection) => (
          <li
            key={connection.id}
            className="flex flex-wrap items-baseline gap-x-2"
          >
            <EngramName node={connection.from} />
            {/*
              Between the two ends, so the line reads as the sentence it is:
              "Beta supersedes Alpha". A reference the payload gave no type
              names itself the way the relations list on an engram page names
              one, rather than leaving the two ends sitting next to each other
              with nothing between them.
            */}
            <span className="rounded bg-slate-100 px-1.5 py-0.5 font-mono text-xs text-slate-600 dark:bg-slate-800 dark:text-slate-300">
              {connection.relType ?? "relates to"}
            </span>
            <EngramName node={connection.to} />
          </li>
        ))}
      </ul>

      {stranded.length > 0 && (
        <ul
          aria-label="Engrams with no connection drawn"
          className="flex flex-wrap gap-x-3 gap-y-1 text-sm"
        >
          {stranded.map((node) => (
            <li key={`${node.domain}/${node.permalink}`}>
              <EngramName node={node} />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/**
 * One end of a connection: named, linked, and faded when it is retired, the
 * same way every engram this app lists is.
 */
function EngramName({ node }: { node: GraphNode }) {
  return (
    <Link
      to={engramRoute(node.domain, node.permalink)}
      aria-label={`${node.title}, ${node.permalink}`}
      className={`text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400 ${
        isRetired(node.status) ? RETIRED_CLASS : ""
      }`}
    >
      {node.title}
    </Link>
  );
}

/** Something the screen has to say that is not a failure. */
function Quiet({ children }: { children: ReactNode }) {
  return (
    <p className="text-sm text-slate-500 dark:text-slate-400">{children}</p>
  );
}
