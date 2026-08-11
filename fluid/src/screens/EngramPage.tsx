/**
 * One engram: the screen this app exists to draw.
 *
 * Two requests make it. The detail payload is the engram itself - its markdown,
 * its frontmatter, and every reference the server parsed out of the body with a
 * flag saying whether the index resolved it. The neighborhood graph is where
 * those references landed, because the detail payload names a target as it was
 * written (a title, usually) and never as an address.
 *
 * So the two are read together rather than shown side by side: the resolver
 * that linkifies the body needs a fact from each, and until the graph lands a
 * wikilink the index resolved is prose rather than a link that guesses.
 *
 * What points back is no longer among them. The graph is capped at a hundred
 * and fifty nodes, which is a cap on the backlinks drawn from it; the panel
 * counts and pages the whole index instead, and asks for nothing when the
 * detail payload's `inboundCount` is already zero. The graph's inbound edges
 * are still read here for the two things they are exact about: the supersedes
 * chain, whose other half only the linker wrote down, and the retire dialog's
 * warning about what would be left dangling.
 *
 * The detail response is cached under `(domain, permalink)` with the checksum
 * it carries, which is the same token its `ETag` carries and the one a later
 * conditional write presents back as `expected_checksum`. Keeping it is what
 * makes editing from this screen possible without a re-read.
 *
 * The observation and relation bullets render once, in the body, in chip
 * form: the written line and its indexed reading are the same line drawn
 * one way, and the details panel deliberately repeats none of it.
 */

import { useQuery } from "@tanstack/react-query";
import { ChevronRight, MoreHorizontal } from "lucide-react";
import { DropdownMenu } from "radix-ui";
import { useMemo, useRef, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";

import { ApiProblem, problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY, fetchDomains } from "../api/domains";
import type { EngramDetail } from "../api/engram";
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
import { DetailsPanel } from "../components/DetailsPanel";
import { EngramActions } from "../components/EngramActions";
import type { EngramActionHandlers } from "../components/EngramActions";
import { LifecycleBanner } from "../components/LifecycleBanner";
import type { LifecycleLink } from "../components/LifecycleBanner";
import { Markdown } from "../components/Markdown";
import { ITEM_CLASSES, MENU_CLASSES } from "../components/menu";
import { MoveDialog } from "../components/MoveDialog";
import { NeighborhoodGraph } from "../components/NeighborhoodGraph";
import { BUTTON, IconButton } from "../components/primitives";
import { RetireDialog } from "../components/RetireDialog";
import { Skeleton } from "../components/Skeleton";
import { useRememberedDisclosure } from "../disclosure";
import { domainRoute, editRoute, engramRoute, graphRoute } from "../paths";
import type { WikilinkResolver } from "../wikilinks";
import { buildWikilinkResolver, innerOf, referenceState } from "../wikilinks";

/** Where the neighborhood section writes down whether it was left open. */
const GRAPH_SECTION_KEY = "fluid.section.graph";

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
          Where this engram lives, and what can be done with it: the address on
          the left, the controls on the right, the title alone on the line
          below with nothing competing for it.

          The trail prints: the details panel and the controls beside it are
          chrome and stay off the page, so this line is what says on paper
          which domain and which folders this document came out of.
        */}
        <div className="flex flex-wrap items-center justify-between gap-2">
          <Breadcrumbs
            crumbs={crumbsOf(engram.domain, engram.permalink, engram.title)}
          />
          {/*
            One thing to do and one place to look for the rest. Editing is what
            somebody came to this page's header for, so it keeps its name and
            its button; everything else is a row in the overflow menu, where
            the destructive one sits alone below a rule. `EngramActions` builds
            the three utilities and hands them over through the ref the menu
            rows and the palette both run; it draws nothing here but the region
            that announces them.
          */}
          <div className="flex flex-wrap items-center gap-2 print:hidden">
            {capabilities.canWrite && (
              <Link
                to={editRoute(engram.domain, engram.permalink)}
                onPointerEnter={prefetchEditor}
                onFocus={prefetchEditor}
                className={BUTTON.primary}
              >
                Edit
              </Link>
            )}
            <DropdownMenu.Root>
              <DropdownMenu.Trigger asChild>
                <IconButton label="More actions" icon={MoreHorizontal} />
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content
                  align="end"
                  sideOffset={6}
                  className={MENU_CLASSES}
                >
                  {capabilities.canWrite && (
                    <DropdownMenu.Item
                      className={ITEM_CLASSES}
                      onSelect={() => {
                        setMoving(true);
                      }}
                    >
                      Move
                    </DropdownMenu.Item>
                  )}
                  <DropdownMenu.Item
                    className={ITEM_CLASSES}
                    onSelect={() => {
                      utilities.current?.download();
                    }}
                  >
                    Download as Markdown
                  </DropdownMenu.Item>
                  <DropdownMenu.Item
                    className={ITEM_CLASSES}
                    onSelect={() => {
                      utilities.current?.share();
                    }}
                  >
                    Share link
                  </DropdownMenu.Item>
                  <DropdownMenu.Item
                    className={ITEM_CLASSES}
                    onSelect={() => {
                      utilities.current?.print();
                    }}
                  >
                    Print view
                  </DropdownMenu.Item>
                  {/*
                    The rule and the retirement are one piece: a reader who
                    may not write sees neither, rather than a menu ending in a
                    divider with nothing under it.
                  */}
                  {capabilities.canWrite && (
                    <>
                      <DropdownMenu.Separator className="my-1 h-px bg-slate-200 dark:bg-slate-700" />
                      <DropdownMenu.Item
                        className={`${ITEM_CLASSES} text-red-700 dark:text-red-300`}
                        onSelect={() => {
                          setRetiring(true);
                        }}
                      >
                        Retire
                      </DropdownMenu.Item>
                    </>
                  )}
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
            <EngramActions engram={engram} handlers={utilities} />
          </div>
        </div>
        <h1 id="engram-title" className="text-display">
          {engram.title}
        </h1>
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
            {/*
              The heading above is the page's rendering of the title, so the
              body's own opening `# Title` folds away rather than repeating
              it.
            */}
            <Markdown
              source={engram.content}
              wikilinks={wikilinks}
              foldTitle={engram.title}
            />
          </article>
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
        <aside className="flex flex-col gap-6 print:hidden">
          <DetailsPanel frontmatter={engram.frontmatter} address={engram.url} />
          <BacklinksPanel
            domain={engram.domain}
            permalink={engram.permalink}
            inboundCount={engram.inboundCount}
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

/**
 * The neighborhood, one hop out, folded away until it is asked for.
 *
 * Folded by default for two reasons that point the same way. The drawing is the
 * heaviest thing this page can load and it arrives only when the section opens,
 * so a reader who came for the prose never pays for it; and the picture is a
 * detour from the engram, which is what this page is for. Opened, it costs
 * nothing on the wire either: it reads the same neighborhood under the same
 * cache key the backlinks panel already read.
 *
 * A reader who does open it is a reader who reads this way, so the section
 * remembers: the default is closed, and the choice against it survives the
 * visit.
 */
function GraphSection({
  domain,
  permalink,
}: {
  domain: string;
  permalink: string;
}) {
  const [open, toggle] = useRememberedDisclosure(GRAPH_SECTION_KEY);

  return (
    <section aria-labelledby="engram-graph">
      <div className="mb-2 flex flex-wrap items-baseline justify-between gap-3">
        <h2 id="engram-graph" className="text-section">
          Graph
        </h2>
        {open && (
          <Link
            to={graphRoute(domain, permalink)}
            className="text-sm text-accent-700 underline underline-offset-2 hover:no-underline dark:text-accent-300"
          >
            Open the full view
          </Link>
        )}
      </div>
      <button
        type="button"
        aria-expanded={open}
        aria-controls="engram-graph-panel"
        onClick={toggle}
        className={`${BUTTON.ghost} inline-flex items-center gap-1.5`}
      >
        <ChevronRight
          aria-hidden="true"
          size={14}
          strokeWidth={1.75}
          className={
            open ? "rotate-90 transition-transform" : "transition-transform"
          }
        />
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
