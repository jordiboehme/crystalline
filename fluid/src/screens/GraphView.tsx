/**
 * The neighborhood graph, full screen.
 *
 * The URL is the state, as it is on search: the anchor and the depth live in
 * the search params under the API's own names, so a picture is a link somebody
 * can send and the back button walks the hops rather than the clicks. Nothing
 * on this screen holds a setting the address bar does not show.
 *
 * An address with no anchor is not a failure. `/graph` on its own is somewhere
 * a reader can arrive from the address bar with a reasonable question, so it
 * answers with what an anchor is and where to find one, rather than with an
 * error about a parameter.
 *
 * The drawing, the bounded-view notice and the reading of the payload all live
 * in `NeighborhoodGraph`, which the engram page embeds as well: this screen is
 * the address, the depth and the frame around it.
 */

import { Link, useSearchParams } from "react-router";

import { parseCrystallineAddress } from "../api/engram";
import { GRAPH_DEPTHS, NEIGHBORHOOD_DEPTH, readGraphDepth } from "../api/graph";
import { NeighborhoodGraph } from "../components/NeighborhoodGraph";
import { domainRoute } from "../paths";

export default function GraphView() {
  const [params, setParams] = useSearchParams();
  const address = params.get("anchor") ?? "";
  const anchor = parseCrystallineAddress(address);
  const depth = readGraphDepth(params.get("depth"));

  return (
    <div className="flex flex-col gap-6">
      <header className="flex flex-wrap items-baseline justify-between gap-3">
        <div className="flex flex-col gap-1">
          <h1 className="text-xl font-semibold">Graph</h1>
          {anchor && (
            <p className="flex flex-wrap items-center gap-x-3 text-sm text-slate-500 dark:text-slate-400">
              <Link
                to={domainRoute(anchor.domain)}
                className="underline underline-offset-2 hover:no-underline"
              >
                {anchor.domain}
              </Link>
              <span className="font-mono text-xs">{address}</span>
            </p>
          )}
        </div>
        {anchor && (
          <DepthChoice
            depth={depth}
            onChange={(next) => {
              const updated = new URLSearchParams(params);
              // The default is not written: a URL says what was chosen, and one
              // hop is what a neighborhood is when nobody chose.
              if (next === NEIGHBORHOOD_DEPTH) {
                updated.delete("depth");
              } else {
                updated.set("depth", String(next));
              }
              // A step rather than a replacement: widening the picture is a
              // deliberate move, and the reader can take it back.
              setParams(updated);
            }}
          />
        )}
      </header>

      {anchor ? (
        <NeighborhoodGraph anchor={anchor} depth={depth} height="h-[70vh]" />
      ) : (
        <NoAnchor address={address} />
      )}
    </div>
  );
}

/** How many hops out. Two is as far as the endpoint walks. */
function DepthChoice({
  depth,
  onChange,
}: {
  depth: number;
  onChange: (depth: number) => void;
}) {
  return (
    <span className="flex items-center gap-2">
      <label
        htmlFor="graph-depth"
        className="text-xs text-slate-500 dark:text-slate-400"
      >
        Depth
      </label>
      <select
        id="graph-depth"
        value={String(depth)}
        onChange={(event) => {
          onChange(Number(event.target.value));
        }}
        className="rounded border border-slate-300 bg-white px-2 py-1 text-sm dark:border-slate-700 dark:bg-slate-900"
      >
        {GRAPH_DEPTHS.map((option) => (
          <option key={option} value={String(option)}>
            {option === 1 ? "1 hop" : `${String(option)} hops`}
          </option>
        ))}
      </select>
    </span>
  );
}

/**
 * The screen with nothing to draw around.
 *
 * Two things it can be: nobody named an engram, or what was named is not an
 * address. Both get told what an anchor is, because in either case the way
 * forward is the same one.
 */
function NoAnchor({ address }: { address: string }) {
  return (
    <div className="flex flex-col items-start gap-3">
      <p className="text-sm text-slate-500 dark:text-slate-400">
        {address === ""
          ? "No engram chosen yet. A graph is drawn around one engram, named by its address."
          : `"${address}" is not an engram address, so there is nothing to draw around it.`}
      </p>
      <p className="text-sm text-slate-500 dark:text-slate-400">
        An address looks like{" "}
        <span className="font-mono text-xs">
          crystalline://domain/permalink
        </span>
        . Every engram page has one to copy, and its Graph section opens this
        screen already pointed at it.
      </p>
      <Link
        to="/search"
        className="text-sm text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
      >
        Find an engram to start from
      </Link>
    </div>
  );
}
