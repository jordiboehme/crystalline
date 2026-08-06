/**
 * One engram: the screen this app exists to draw.
 *
 * Two requests make it. The detail payload is the engram itself - its markdown,
 * its frontmatter, and every reference the server parsed out of the body with a
 * flag saying whether the index resolved it. The neighborhood graph is where
 * those references landed and what points back, because the detail payload
 * names a target as it was written (a title, usually) and never as an address,
 * and its inbound block is a sample capped at five rather than the set.
 *
 * So the two are read together rather than shown side by side: the resolver
 * that linkifies the body needs a fact from each, and until the graph lands a
 * wikilink the index resolved is prose rather than a link that guesses.
 *
 * The detail response is cached under `(domain, permalink)` with the checksum
 * it carries, which is the same token its `ETag` carries and the one a later
 * conditional write presents back as `expected_checksum`. Keeping it is what
 * makes editing from this screen possible without a re-read.
 *
 * The observation and relation bullets appear twice on purpose: once in the
 * body, because they are lines of the markdown somebody wrote and cutting them
 * out would show a document nobody has, and once as lists, which is where the
 * category, the tags, the context and whether the target resolved are legible.
 * The two are the same lines read two ways rather than a duplicate.
 */

import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { Link, useParams } from "react-router";

import { ApiProblem } from "../api/client";
import type {
  EngramDetail,
  EngramObservation,
  EngramReference,
} from "../api/engram";
import { engramDetailKey, fetchEngramDetail } from "../api/engram";
import {
  NEIGHBORHOOD_DEPTH,
  backlinksTo,
  fetchGraph,
  graphKey,
} from "../api/graph";
import { BacklinksPanel } from "../components/BacklinksPanel";
import { FrontmatterPanel } from "../components/FrontmatterPanel";
import { LifecycleBanner } from "../components/LifecycleBanner";
import type { LifecycleLink } from "../components/LifecycleBanner";
import { Markdown } from "../components/Markdown";
import { domainRoute } from "../paths";
import type { WikilinkResolver } from "../wikilinks";
import { buildWikilinkResolver, innerOf } from "../wikilinks";

/** How long the copy button keeps saying it worked. */
const COPIED_FOR_MS = 2000;

export default function EngramPage() {
  const params = useParams();
  const domain = params.domain ?? "";
  // A permalink is a path of its own, so it arrives through the splat.
  const permalink = params["*"] ?? "";

  const detail = useQuery({
    queryKey: engramDetailKey(domain, permalink),
    queryFn: () => fetchEngramDetail(domain, permalink),
  });
  const graph = useQuery({
    queryKey: graphKey(domain, permalink, NEIGHBORHOOD_DEPTH),
    queryFn: () => fetchGraph(domain, permalink),
    // Only once there is an engram to have a neighborhood. A wrong address
    // would otherwise be answered by two 404s where one says everything.
    enabled: detail.isSuccess,
  });

  const wikilinks = useMemo(
    () =>
      detail.data ? buildWikilinkResolver(detail.data, graph.data) : undefined,
    [detail.data, graph.data],
  );

  if (isMissing(detail.error)) {
    return <EngramNotFound domain={domain} permalink={permalink} />;
  }
  if (detail.error) {
    return (
      <p
        role="alert"
        className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
      >
        {detail.error instanceof ApiProblem
          ? detail.error.detail
          : detail.error.message}
      </p>
    );
  }
  if (!detail.data || !wikilinks) {
    return (
      <p className="text-sm text-slate-500 dark:text-slate-400">
        Loading the engram
      </p>
    );
  }

  const engram = detail.data;
  return (
    <div className="flex flex-col gap-6">
      <header className="flex flex-col gap-2">
        <h1 id="engram-title" className="text-xl font-semibold">
          {engram.title}
        </h1>
        <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-slate-500 dark:text-slate-400">
          <Link
            to={domainRoute(engram.domain)}
            className="underline underline-offset-2 hover:no-underline"
          >
            {engram.domain}
          </Link>
          <span className="font-mono text-xs">{engram.permalink}</span>
          <CopyAddressButton address={engram.url} />
        </p>
      </header>

      <LifecycleBanner
        status={engram.frontmatter.status}
        staleAfter={engram.frontmatter.staleAfter}
        supersededBy={chain(engram, wikilinks, "superseded_by")}
        supersedes={chain(engram, wikilinks, "supersedes")}
      />

      {/*
        The body leads and the panels follow it: one column on a narrow screen,
        with the panels under what they describe, and a column beside it once
        there is room for one.
      */}
      <div className="grid gap-8 lg:grid-cols-[minmax(0,1fr)_18rem]">
        <div className="flex min-w-0 flex-col gap-8">
          <article aria-labelledby="engram-title">
            <Markdown source={engram.content} wikilinks={wikilinks} />
          </article>
          <Observations observations={engram.observations} />
          <Relations relations={engram.relations} resolve={wikilinks} />
        </div>
        <aside className="flex flex-col gap-4">
          <FrontmatterPanel frontmatter={engram.frontmatter} />
          <BacklinksPanel
            backlinks={backlinksTo(graph.data, engram.domain, engram.permalink)}
            pending={graph.isPending}
            error={graph.error}
            truncated={graph.data?.truncated ?? false}
          />
        </aside>
      </div>
    </div>
  );
}

/**
 * One direction of the supersedes chain, read off the engram's own relations.
 *
 * Outbound only, which is what the engram itself asserts. A successor that
 * declares the other half of the pair shows up as a backlink instead, where it
 * belongs: this banner speaks for the engram being read.
 */
function chain(
  engram: EngramDetail,
  resolve: WikilinkResolver,
  relType: string,
): LifecycleLink[] {
  return engram.relations
    .filter((relation) => relation.relType === relType)
    .map((relation) => {
      const resolution = resolve(innerOf(relation.target));
      return {
        label: relation.target.target,
        href: resolution?.kind === "resolved" ? resolution.href : null,
      };
    });
}

/** The observation bullets, as the structure they are rather than as prose. */
function Observations({ observations }: { observations: EngramObservation[] }) {
  if (observations.length === 0) {
    return null;
  }
  return (
    <section aria-labelledby="engram-observations">
      <h2 id="engram-observations" className="mb-2 text-lg font-semibold">
        Observations
      </h2>
      <ul className="flex flex-col gap-2 text-sm">
        {observations.map((observation) => (
          <li
            key={`${String(observation.line)}-${observation.content}`}
            className="flex flex-wrap items-baseline gap-2"
          >
            {observation.category !== null && (
              // In the brackets it was written in, which is what tells it
              // apart from the same word used as a type or a tag.
              <span className="rounded bg-slate-100 px-1.5 py-0.5 font-mono text-xs text-slate-600 dark:bg-slate-800 dark:text-slate-300">
                [{observation.category}]
              </span>
            )}
            <span>{observation.content}</span>
            {observation.tags.map((tag) => (
              <span
                key={tag}
                className="text-xs text-slate-500 dark:text-slate-400"
              >
                #{tag}
              </span>
            ))}
            {observation.context !== null && (
              <span className="text-xs text-slate-500 dark:text-slate-400">
                ({observation.context})
              </span>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}

/**
 * The relation bullets. A target the index resolved and the graph placed is a
 * link; one it did not is named and marked, never linked somewhere invented.
 */
function Relations({
  relations,
  resolve,
}: {
  relations: EngramReference[];
  resolve: WikilinkResolver;
}) {
  if (relations.length === 0) {
    return null;
  }
  return (
    <section aria-labelledby="engram-relations">
      <h2 id="engram-relations" className="mb-2 text-lg font-semibold">
        Relations
      </h2>
      <ul className="flex flex-col gap-2 text-sm">
        {relations.map((relation) => {
          const inner = innerOf(relation.target);
          const resolution = resolve(inner);
          return (
            <li
              key={`${String(relation.line)}-${relation.relType ?? ""}-${inner}`}
              className="flex flex-wrap items-baseline gap-2"
            >
              <span className="rounded bg-slate-100 px-1.5 py-0.5 font-mono text-xs text-slate-600 dark:bg-slate-800 dark:text-slate-300">
                {relation.relType ?? "relates to"}
              </span>
              {resolution?.kind === "resolved" ? (
                <Link
                  to={resolution.href}
                  className="text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
                >
                  {relation.target.target}
                </Link>
              ) : relation.resolved ? (
                <span>{relation.target.target}</span>
              ) : (
                <span
                  title="not resolved"
                  className="underline decoration-dotted underline-offset-2 opacity-70"
                >
                  {relation.target.target}
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/**
 * Hand the engram's address to the clipboard.
 *
 * `crystalline://domain/permalink` rather than the browser's URL: it is what
 * this engram is called everywhere else, so it is what an agent, a MANIFEST or
 * another engram can be given.
 */
function CopyAddressButton({ address }: { address: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");

  useEffect(() => {
    if (state !== "copied") {
      return;
    }
    const timer = setTimeout(() => {
      setState("idle");
    }, COPIED_FOR_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [state]);

  return (
    <button
      type="button"
      // Named by what it does rather than by what it says, so the confirmation
      // replacing the label does not rename the control under a reader.
      aria-label="Copy address"
      title={address}
      className="rounded border border-slate-300 px-2 py-0.5 text-xs hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-sky-500 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
      onClick={() => {
        void (async () => {
          try {
            await navigator.clipboard.writeText(address);
            setState("copied");
          } catch {
            // A browser that refuses the clipboard is not a failure of the
            // page: the address is in the button's tooltip either way, and
            // saying so beats a button that silently does nothing.
            setState("failed");
          }
        })();
      }}
    >
      {state === "copied"
        ? "Copied"
        : state === "failed"
          ? "Copy refused"
          : "Copy address"}
    </button>
  );
}

/** The wrong-address screen, which is not the same thing as an empty engram. */
function EngramNotFound({
  domain,
  permalink,
}: {
  domain: string;
  permalink: string;
}) {
  return (
    <div className="flex flex-col items-start gap-3">
      <h1 className="text-xl font-semibold">Engram not found</h1>
      <p className="text-sm">
        No engram at {`"${permalink}"`} in {`"${domain}"`}.
      </p>
      <Link
        to={domainRoute(domain)}
        className="text-sm text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
      >
        See what {domain} does hold
      </Link>
    </div>
  );
}

/** Whether this failure is the server saying there is nothing at that address. */
function isMissing(error: unknown): boolean {
  return error instanceof ApiProblem && error.status === 404;
}
