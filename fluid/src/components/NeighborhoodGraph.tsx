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
 * The engrams are listed under the drawing as links as well. A canvas is one
 * opaque element to a screen reader and untabbable to a keyboard, and the
 * neighborhood is worth having either way, so the list is the same answer in
 * the form anything can read.
 */

import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Suspense, lazy, useCallback, useMemo } from "react";
import { Link, useNavigate } from "react-router";

import { ApiProblem } from "../api/client";
import { crystallineAddress } from "../api/engram";
import { fetchGraph, graphKey } from "../api/graph";
import type { GraphAnchor } from "../graphElements";
import { graphElements } from "../graphElements";
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
        The neighborhood could not be read, so there is nothing to draw.
      </p>
    );
  }

  const neighborhood = graph.data;
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

      <ul
        aria-label="Engrams in this neighborhood"
        className="flex flex-wrap gap-x-3 gap-y-1 text-sm"
      >
        {neighborhood.nodes.map((node) => (
          <li
            key={`${node.domain}/${node.permalink}`}
            className={isRetired(node.status) ? RETIRED_CLASS : undefined}
          >
            <Link
              to={engramRoute(node.domain, node.permalink)}
              aria-label={`${node.title}, ${node.permalink}`}
              className="text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
            >
              {node.title}
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** Something the screen has to say that is not a failure. */
function Quiet({ children }: { children: ReactNode }) {
  return (
    <p className="text-sm text-slate-500 dark:text-slate-400">{children}</p>
  );
}
