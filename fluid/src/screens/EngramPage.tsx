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
import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";

import { ApiProblem, problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type {
  EngramDetail,
  EngramObservation,
  EngramReference,
} from "../api/engram";
import { engramDetailKey, fetchEngramDetail } from "../api/engram";
import type { Backlink } from "../api/graph";
import {
  NEIGHBORHOOD_DEPTH,
  backlinksTo,
  fetchGraph,
  graphKey,
} from "../api/graph";
import { useAuth } from "../auth/AuthContext";
import { NO_COMMANDS, useRegisterCommands } from "../commands";
import type { PaletteCommand } from "../commands";
import { AgentsEye } from "../components/AgentsEye";
import { BacklinksPanel } from "../components/BacklinksPanel";
import { Breadcrumbs, crumbsOf } from "../components/Breadcrumbs";
import { EngramActions } from "../components/EngramActions";
import type { EngramActionHandlers } from "../components/EngramActions";
import { FrontmatterPanel } from "../components/FrontmatterPanel";
import { LifecycleBanner } from "../components/LifecycleBanner";
import type { LifecycleLink } from "../components/LifecycleBanner";
import { Markdown } from "../components/Markdown";
import { MoveDialog } from "../components/MoveDialog";
import { NeighborhoodGraph } from "../components/NeighborhoodGraph";
import { ReferenceLink } from "../components/ReferenceLink";
import { RetireDialog } from "../components/RetireDialog";
import { Skeleton } from "../components/Skeleton";
import { domainRoute, editRoute, engramRoute, graphRoute } from "../paths";
import type { WikilinkResolver } from "../wikilinks";
import { buildWikilinkResolver, innerOf, referenceState } from "../wikilinks";

/** How long the copy button keeps saying it worked. */
const COPIED_FOR_MS = 2000;

/** Warm the editor chunk while the pointer is still on its way to the click. */
function prefetchEditor(): void {
  void import("./EngramEditor");
}

export default function EngramPage() {
  const params = useParams();
  const domain = params.domain ?? "";
  // A permalink is a path of its own, so it arrives through the splat.
  const permalink = params["*"] ?? "";
  const { capabilities } = useAuth();
  const navigate = useNavigate();
  const [retiring, setRetiring] = useState(false);
  const [moving, setMoving] = useState(false);
  // The utility three, as `EngramActions` runs them: the palette rows below
  // reach through this rather than repeating the clipboard and blob calls.
  const utilities = useRef<EngramActionHandlers | null>(null);

  // The listing the sidebar already read, under the same key: opening the
  // move dialog's domain picker costs nothing on the wire.
  const domains = useQuery({
    queryKey: DOMAINS_QUERY_KEY,
    queryFn: fetchDomains,
  });

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

  /*
   * What this screen offers the palette: the same things its own buttons do,
   * gated the same way. Built here rather than below the guards because a
   * hook may not sit behind a return, and nothing is registered until there
   * is an engram for the actions to act on.
   */
  const loaded = detail.data;
  const commands = useMemo<readonly PaletteCommand[]>(() => {
    if (!loaded) {
      return NO_COMMANDS;
    }
    // The writes lead, because they are what somebody opens a palette to do
    // that a link would not already have done for them.
    const writes: PaletteCommand[] = capabilities.canWrite
      ? [
          {
            id: "edit",
            title: "Edit engram",
            run: () => {
              void navigate(editRoute(loaded.domain, loaded.permalink));
            },
          },
          {
            id: "retire",
            title: "Retire engram",
            run: () => {
              setRetiring(true);
            },
          },
          {
            id: "move",
            title: "Move engram",
            run: () => {
              setMoving(true);
            },
          },
        ]
      : [];
    return [
      ...writes,
      {
        id: "download",
        title: "Download this engram as Markdown",
        run: () => {
          utilities.current?.download();
        },
      },
      {
        id: "share",
        title: "Share link to this engram",
        run: () => {
          utilities.current?.share();
        },
      },
      {
        id: "print",
        title: "Print this engram",
        run: () => {
          utilities.current?.print();
        },
      },
    ];
  }, [capabilities.canWrite, loaded, navigate]);
  useRegisterCommands(commands);

  if (isMissing(detail.error)) {
    return <EngramNotFound domain={domain} permalink={permalink} />;
  }
  if (detail.error) {
    return (
      <p
        role="alert"
        className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
      >
        {problemDetail(detail.error)}
      </p>
    );
  }
  if (!detail.data || !wikilinks) {
    return <Skeleton label="Loading the engram" rows={6} />;
  }

  const engram = detail.data;
  const backlinks = backlinksTo(graph.data, engram.domain, engram.permalink);
  return (
    <div className="flex flex-col gap-6">
      <header className="flex flex-col gap-2">
        {/*
          Where this engram lives, above its name. The trail prints: the
          details panel and every control below are chrome and stay off the
          page, so this line is what says on paper which domain and which
          folders this document came out of.
        */}
        <Breadcrumbs
          crumbs={crumbsOf(engram.domain, engram.permalink, engram.title)}
        />
        <h1 id="engram-title" className="text-display">
          {engram.title}
        </h1>
        {/*
          The controls, which are chrome: they stay off the printed page,
          where the trail above and the body are the whole document.
        */}
        <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-slate-500 print:hidden dark:text-slate-400">
          <CopyAddressButton address={engram.url} />
          {capabilities.canWrite && (
            <>
              <Link
                to={editRoute(engram.domain, engram.permalink)}
                onPointerEnter={prefetchEditor}
                onFocus={prefetchEditor}
                className="rounded border border-slate-300 px-2 py-0.5 text-xs hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
              >
                Edit
              </Link>
              <button
                type="button"
                onClick={() => {
                  setRetiring(true);
                }}
                className="rounded border border-slate-300 px-2 py-0.5 text-xs hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
              >
                Retire
              </button>
              <button
                type="button"
                onClick={() => {
                  setMoving(true);
                }}
                className="rounded border border-slate-300 px-2 py-0.5 text-xs hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
              >
                Move
              </button>
            </>
          )}
          <EngramActions engram={engram} handlers={utilities} />
        </p>
      </header>

      <LifecycleBanner
        status={engram.frontmatter.status}
        staleAfter={engram.frontmatter.staleAfter}
        supersededBy={chain(
          engram,
          wikilinks,
          backlinks,
          "superseded_by",
          "supersedes",
        )}
        supersedes={chain(
          engram,
          wikilinks,
          backlinks,
          "supersedes",
          "superseded_by",
        )}
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
          <div className="print:hidden">
            <GraphSection domain={engram.domain} permalink={engram.permalink} />
          </div>
          <div className="print:hidden">
            <AgentsEye
              domain={engram.domain}
              salience={engram.frontmatter.salience}
              content={engram.content}
            />
          </div>
        </div>
        <aside className="flex flex-col gap-4 print:hidden">
          <FrontmatterPanel frontmatter={engram.frontmatter} />
          <BacklinksPanel
            backlinks={backlinks}
            pending={graph.isPending}
            error={graph.error}
            truncated={graph.data?.truncated ?? false}
          />
        </aside>
      </div>
      {retiring && (
        <RetireDialog
          engram={engram}
          backlinks={backlinks}
          onClose={() => {
            setRetiring(false);
          }}
        />
      )}
      {moving && (
        <MoveDialog
          engram={engram}
          domains={(domains.data?.domains ?? []).map((entry) => entry.name)}
          onClose={() => {
            setMoving(false);
          }}
        />
      )}
    </div>
  );
}

/**
 * One direction of the supersedes chain.
 *
 * Both halves of it, because either end may be the one that wrote the relation
 * down: this engram saying `- superseded_by [[Beta]]`, or Beta saying
 * `- supersedes [[Alpha]]` from its own side. Only the first is in this
 * engram's payload, so the second is read off the inbound edges of the graph,
 * where the direction is inverted: an inbound `supersedes` means the other
 * engram replaced this one, which is this engram's `superseded_by`.
 *
 * An engram whose successor states it from both sides appears once, because
 * both halves key by the same address.
 */
function chain(
  engram: EngramDetail,
  resolve: WikilinkResolver,
  backlinks: Backlink[],
  relType: string,
  inboundRelType: string,
): LifecycleLink[] {
  const outbound: LifecycleLink[] = engram.relations
    .filter((relation) => relation.relType === relType)
    .map((relation) => {
      const resolution = resolve(innerOf(relation.target));
      return {
        label: relation.target.target,
        href: resolution?.kind === "resolved" ? resolution.href : null,
        state: referenceState(resolution, relation.resolved),
      };
    });

  // An inbound edge exists because the index resolved it, and the node it came
  // from carries its own address, so there is nothing pending about it.
  const inbound: LifecycleLink[] = backlinks
    .filter((backlink) => backlink.relTypes.includes(inboundRelType))
    .map((backlink) => ({
      label: backlink.node.title,
      href: engramRoute(backlink.node.domain, backlink.node.permalink),
      state: "resolved" as const,
    }));

  // Keyed by address, which is what makes the two halves of one pair the same
  // entry. A pending end has no address yet, but neither does the inbound half
  // exist yet: both wait on the same graph request, so there is no window in
  // which one pair could key two ways.
  const merged = new Map<string, LifecycleLink>();
  for (const link of [...outbound, ...inbound]) {
    const key = link.href ?? link.label;
    if (!merged.has(key)) {
      merged.set(key, link);
    }
  }
  return [...merged.values()];
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
              <ReferenceLink
                label={relation.target.target}
                href={resolution?.kind === "resolved" ? resolution.href : null}
                state={referenceState(resolution, relation.resolved)}
              />
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/**
 * The neighborhood, one hop out, folded away until it is asked for.
 *
 * Folded by default for two reasons that point the same way. The drawing is the
 * heaviest thing this page can load and it arrives only when the section opens,
 * so a reader who came for the prose never pays for it; and the picture is a
 * detour from the engram, which is what this page is for. Opened, it costs
 * nothing on the wire either: it reads the same neighborhood under the same
 * cache key the backlinks panel already read.
 */
function GraphSection({
  domain,
  permalink,
}: {
  domain: string;
  permalink: string;
}) {
  const [open, setOpen] = useState(false);

  return (
    <section aria-labelledby="engram-graph">
      <div className="mb-2 flex flex-wrap items-baseline justify-between gap-3">
        <h2 id="engram-graph" className="text-lg font-semibold">
          Graph
        </h2>
        {open && (
          <Link
            to={graphRoute(domain, permalink)}
            className="text-sm text-sky-700 underline underline-offset-2 hover:no-underline dark:text-sky-400"
          >
            Open the full view
          </Link>
        )}
      </div>
      <button
        type="button"
        aria-expanded={open}
        aria-controls="engram-graph-panel"
        onClick={() => {
          setOpen((was) => !was);
        }}
        className="rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
      >
        {open ? "Hide the neighborhood" : "Show the neighborhood"}
      </button>
      <div id="engram-graph-panel" className="mt-3">
        {open && (
          <NeighborhoodGraph
            anchor={{ domain, permalink }}
            depth={NEIGHBORHOOD_DEPTH}
            height="h-80"
          />
        )}
      </div>
    </section>
  );
}

/**
 * Hand the engram's address to the clipboard.
 *
 * `crystalline://domain/permalink` rather than the browser's URL: it is what
 * this engram is called everywhere else, so it is what an agent, a MANIFEST or
 * another engram can be given.
 *
 * The outcome is announced in a live region beside the button rather than
 * written into the button's own label. A control that renames itself is a
 * control a reader navigating by name loses track of, and a label that changes
 * silently is no announcement at all: the region is in the document from the
 * start and empty, so the text arriving in it is what gets read out.
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
    <span className="inline-flex items-center gap-2">
      <button
        type="button"
        title={address}
        className="rounded border border-slate-300 px-2 py-0.5 text-xs hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none dark:border-slate-700 dark:hover:bg-slate-800"
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
        Copy address
      </button>
      <span
        role="status"
        aria-live="polite"
        aria-label="Copy address result"
        className="text-xs text-slate-500 dark:text-slate-400"
      >
        {state === "copied"
          ? "Copied"
          : state === "failed"
            ? "Copy refused"
            : ""}
      </span>
    </span>
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
