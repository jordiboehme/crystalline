/**
 * Review: what a domain's Reflection is waiting on.
 *
 * Open threads first, because those are people waiting: split into the ones
 * waiting for people (an agent spoke last) and the ones waiting for the
 * agent (a person spoke last), each grouped by page and oldest first with
 * its age, in the server's own order. Below them, the reflect queue an agent
 * works through (`reflect`), read-only in the way Maintenance shows the
 * evolve queue: a person reads here what is owed, and follows a row to the
 * page or the engram to do it. The queue's review rows are the threads
 * above, so they are not drawn twice.
 *
 * Nothing here writes, so the thread cards carry no Answer or Resolve. The
 * screen draws the server's `anchor.state` as it is: it has no section text
 * to judge an anchor against.
 */

import { useQuery } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { Link, useParams } from "react-router";

import { problemDetail } from "../api/client";
import type { QueueItem, ReviewGroup, TreeNode } from "../api/reflection";
import {
  fetchReflectionTree,
  fetchReview,
  reflectionAssetUrl,
  reflectionReviewKey,
  reflectionTreeKey,
} from "../api/reflection";
import { useAuth } from "../auth/AuthContext";
import { useReflectionStatus } from "../components/ReflectionModeSwitch";
import { ReflectionStateNotice } from "../components/ReflectionStateNotice";
import { ThreadCard } from "../components/ReflectionThreads";
import { SourceRow } from "../components/SourceRow";
import { ReviewDiff } from "../components/ReviewDiff";
import { Loading } from "../components/Loading";
import { Chip, RAIL_LABEL } from "../components/primitives";
import { plural } from "../format";
import { reflectionPageRoute } from "../paths";
import { ageText } from "../reflectionText";
import {
  hiddenSourcesText,
  sourceKey,
  unavailableSourcesText,
} from "../reflectionSources";

const LINK =
  "text-link underline decoration-(--link-line) decoration-1 underline-offset-[3px] hover:no-underline";
/** One plain label per queue `what`; an unknown value is put into words. */
const WHAT_LABEL: Record<string, string> = {
  new: "New section",
  changed: "Section edited",
  removed: "Section removed",
  pact_changed: "Pact edited",
  source_changed: "A source changed",
  source_retired: "A source was retired",
  source_missing: "A source is gone",
  source_unavailable: "Sources not checked here",
  source_too_wide: "Cites a domain fewer people read",
  off_structure: "Outside the pact's structure",
  asset_missing: "Missing file",
  engram_attachment: "Links to an engram file",
  asset_unreferenced: "File no page uses",
  host_moved: "Domain moved to this instance",
  unreflected: "No page yet",
};

/** "just now" or "2 days ago". */
function agoText(seconds: number): string {
  return seconds < 60 ? "just now" : `${ageText(seconds)} ago`;
}

/**
 * The folder names of the pact's structure globs. A glob arrives as the
 * pact wrote it, often with a backticked folder and a few words after it
 * (`guides/` for how to do things), so only the leading folder is kept.
 */
function structureFolders(globs: string[]): string[] {
  return globs
    .map((glob) => /^[\s`'"]*([^\s`'"]+)/.exec(glob)?.[1] ?? "")
    .map((name) => name.replace(/[/*]+$/, ""))
    .filter((name) => name !== "");
}

function whatLabel(what: string): string {
  const known = WHAT_LABEL[what];
  if (known !== undefined) {
    return known;
  }
  const words = what.replace(/_/g, " ").trim();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

const MUTED = "text-sm text-muted";

export default function ReflectionReview(): ReactElement {
  const { domain = "" } = useParams();
  const status = useReflectionStatus(domain);
  const review = useQuery({
    queryKey: reflectionReviewKey(domain),
    queryFn: () => fetchReview(domain),
    enabled: status.data?.state === "on",
  });
  // Page titles for the queue's groups: the sidebar reads the same tree, so
  // this is usually already cached. A page the tree does not know shows its path.
  const tree = useQuery({
    queryKey: reflectionTreeKey(domain),
    queryFn: () => fetchReflectionTree(domain),
    enabled: status.data?.state === "on",
  });
  return (
    <div className="flex flex-col gap-8">
      <header className="flex flex-wrap items-center justify-between gap-2">
        <div>
          <h1 className="text-display">Review</h1>
          <p className="text-caption text-slate-500 dark:text-slate-400">
            {`${domain}, Reflection`}
          </p>
        </div>
      </header>
      {status.data && <ReflectionStateNotice status={status.data} />}
      {(status.isPending ||
        (status.data?.state === "on" && review.isPending)) && (
        <Loading label="Loading the review" subject={`${domain} review`} />
      )}
      {review.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(review.error)}
        </p>
      )}
      {review.data && (
        <>
          <section
            aria-labelledby="review-threads"
            className="flex flex-col gap-6"
          >
            <h2 id="review-threads" className="text-section">
              Open threads
            </h2>
            <Waiting
              domain={domain}
              id="review-people"
              title="Waiting for people"
              groups={review.data.waitingForPeople}
            />
            <Waiting
              domain={domain}
              id="review-agent"
              title="Waiting for the agent"
              groups={review.data.waitingForTheAgent}
            />
          </section>
          <section
            aria-labelledby="review-queue"
            className="flex flex-col gap-3"
          >
            <h2 id="review-queue" className="text-section">
              Reflect queue
            </h2>
            <Queue
              domain={domain}
              items={review.data.queue.items}
              titles={titlesOf(tree.data?.nodes ?? [], [
                ...review.data.waitingForPeople,
                ...review.data.waitingForTheAgent,
              ])}
              engramsVisible={status.data?.engramsVisible === true}
              total={review.data.queue.total}
            />
          </section>
        </>
      )}
    </div>
  );
}

function Waiting({
  domain,
  id,
  title,
  groups,
}: {
  domain: string;
  id: string;
  title: string;
  groups: ReviewGroup[];
}): ReactElement {
  const { user } = useAuth();
  return (
    <section aria-labelledby={id} className="flex flex-col">
      <h3 id={id} className={`mb-1 ${RAIL_LABEL}`}>
        {title}
      </h3>
      {groups.length === 0 && <p className={MUTED}>Nobody is waiting.</p>}
      {groups.map((group) => (
        <div key={group.path} className="flex flex-col">
          <Link
            to={reflectionPageRoute(domain, group.path)}
            className={`mt-3 text-[15px] font-semibold ${LINK}`}
          >
            {group.title}
          </Link>
          {group.threads.map((thread) => (
            <div key={thread.id} className="mt-3 border-t border-hair py-4">
              <ThreadCard
                thread={thread}
                heading={null}
                state={thread.anchor.state}
                viewer={user?.name ?? null}
                actions={
                  <p className="text-caption text-muted">
                    <Link
                      to={`${reflectionPageRoute(domain, group.path)}#${thread.id}`}
                      className={LINK}
                    >
                      {agoText(thread.ageSecs)}
                    </Link>
                  </p>
                }
              />
            </div>
          ))}
        </div>
      ))}
    </section>
  );
}

/** Page title by path: the tree's titles first, then the review's own groups. */
function titlesOf(
  nodes: TreeNode[],
  groups: ReviewGroup[],
): Map<string, string> {
  const titles = new Map<string, string>();
  for (const group of groups) {
    titles.set(group.path, group.title);
  }
  const walk = (list: TreeNode[]): void => {
    for (const node of list) {
      if (node.kind === "folder") {
        walk(node.children);
      } else {
        titles.set(node.path, node.title);
      }
    }
  };
  walk(nodes);
  return titles;
}

/**
 * The reflect queue, feedback and update rows only. An item with no path
 * (`asset_unreferenced`, `host_moved`) has no page, so it draws no page link.
 * `QueueDetail` draws the file fields of those items.
 */
function Queue({
  domain,
  items,
  titles,
  total,
  engramsVisible,
}: {
  domain: string;
  items: QueueItem[];
  /** Page titles by path; a page missing here shows its path. */
  titles: Map<string, string>;
  /** Source engrams are named only to a reader who may see engrams. */
  engramsVisible: boolean;
  total: number;
}): ReactElement {
  // An unreflected row is about engrams, so a reader who may not see them is
  // told nothing of it (the server already leaves it out; this is the gate).
  const shown = items.filter(
    (item) =>
      item.kind !== "review" && (engramsVisible || item.what !== "unreflected"),
  );
  // What the page left out, whatever its kind: the review rows it carried
  // count as carried.
  const more = total - items.length;
  // One group per page, in the server's order of first appearance; the items
  // that belong to no page come last.
  const pages = new Map<string, QueueItem[]>();
  for (const item of shown) {
    pages.set(item.path, [...(pages.get(item.path) ?? []), item]);
  }
  const grouped = [...pages.entries()].sort(
    ([a], [b]) => Number(a === "") - Number(b === ""),
  );
  return (
    <>
      {shown.length === 0 && <p className={MUTED}>The queue is empty.</p>}
      <ul aria-label="Pages in the queue" className="flex flex-col gap-6">
        {grouped.map(([path, list]) => {
          const title =
            titles.get(path) ?? (path === "PACT.md" ? "Pact" : path);
          return (
            <li key={path} className="flex flex-col">
              {path === "" ? (
                <h3 className={RAIL_LABEL}>Not on a page</h3>
              ) : (
                <h3 className="text-[15px] font-semibold">
                  <Link to={reflectionPageRoute(domain, path)} className={LINK}>
                    {title}
                  </Link>
                </h3>
              )}
              <ul
                aria-label={
                  path === "" ? "Queue items" : `Queue items for ${title}`
                }
                className="mt-2 flex flex-col"
              >
                {list.map((item) => (
                  <QueueRow
                    key={item.n}
                    domain={domain}
                    item={item}
                    pageTitle={title}
                    engramsVisible={engramsVisible}
                  />
                ))}
              </ul>
            </li>
          );
        })}
      </ul>
      {more > 0 && (
        <p className={MUTED}>{`${String(more)} more in the queue`}</p>
      )}
    </>
  );
}

function QueueRow({
  domain,
  item,
  pageTitle,
  engramsVisible,
}: {
  domain: string;
  item: QueueItem;
  pageTitle: string;
  engramsVisible: boolean;
}): ReactElement {
  const heading = item.heading !== null && item.heading !== "";
  return (
    <li className="border-t border-hair py-3 text-sm">
      <div className="flex flex-wrap items-baseline gap-2">
        <Chip variant="neutral">{whatLabel(item.what)}</Chip>
        {heading && item.path !== "" && (
          <Link
            to={`${reflectionPageRoute(domain, item.path)}${item.section !== null ? `#s-${item.section}` : ""}`}
            className={LINK}
          >
            {item.heading}
          </Link>
        )}
        <span className="text-caption text-muted">{ageText(item.ageSecs)}</span>
      </div>
      {item.changedBy.length > 0 && (
        <p className="text-caption mt-1 text-muted">
          {`changed by ${item.changedBy.join(", ")}`}
        </p>
      )}
      {item.structure.length > 0 && (
        <p className="text-caption mt-1 text-muted">
          {`This page is not in a folder the pact names (${structureFolders(item.structure).join(", ")}).`}
        </p>
      )}
      {item.what === "source_too_wide" && (
        <p className="mt-1">
          This section cites a domain fewer people may read than this one.
          Remove that source and the text drawn from it; the page stays as it is
          until then.
        </p>
      )}
      {item.count !== null && item.what === "unreflected" && (
        <p className="mt-1">{`${plural(item.count, "engram", "engrams")}${item.folder !== null && item.folder !== "" ? ` under ${item.folder}` : ""} ${item.count === 1 ? "has" : "have"} no page yet.`}</p>
      )}
      <QueueDetail
        domain={domain}
        item={item}
        engramsVisible={engramsVisible}
      />
      {item.diff !== null && (
        <ReviewDiff
          diff={item.diff}
          label={
            heading
              ? `Changes in ${item.heading ?? ""}, ${pageTitle}`
              : `Changes in ${pageTitle}`
          }
        />
      )}
      {engramsVisible &&
        item.what === "source_unavailable" &&
        item.sourceDomain !== null &&
        item.count !== null && (
          <p className="mt-1">
            {unavailableSourcesText(item.count, item.sourceDomain)}
          </p>
        )}
      {engramsVisible && item.sources !== null && item.sources.length > 0 && (
        <ul className="mt-1 flex flex-col gap-1">
          {item.sources.map((source) => (
            <SourceRow
              key={sourceKey(domain, source)}
              pageDomain={domain}
              source={source}
              className="flex items-center gap-2"
            />
          ))}
        </ul>
      )}
      {engramsVisible && item.hiddenSources > 0 && (
        <p className="text-caption mt-1 text-muted">
          {hiddenSourcesText(item.hiddenSources)}
        </p>
      )}
    </li>
  );
}

/** `a, b and c` for a list of two or more, the single word for one. */
function joinWords(parts: string[]): string {
  return parts.length < 2
    ? (parts[0] ?? "")
    : `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1] ?? ""}`;
}

/**
 * The file fields of an item. Missing files are gone, so plain text; engram
 * attachment targets name engram files, so they are plain text too and only
 * for a reader who may see engrams; an unreferenced file opens in a new tab.
 */
function QueueDetail({
  domain,
  item,
  engramsVisible,
}: {
  domain: string;
  item: QueueItem;
  engramsVisible: boolean;
}): ReactElement | null {
  const names = (list: string[], label: string) =>
    list.length > 0 && (
      <ul aria-label={label} className="mt-1 flex flex-col gap-0.5">
        {list.map((name) => (
          <li key={name} className="font-mono text-xs break-all">
            {name}
          </li>
        ))}
      </ul>
    );
  const moved = item.onlyElsewhere;
  const counts =
    moved === null
      ? []
      : [
          moved.engrams > 0 && plural(moved.engrams, "engram", "engrams"),
          moved.attachments > 0 &&
            plural(moved.attachments, "attachment", "attachments"),
          moved.pages > 0 && plural(moved.pages, "page", "pages"),
          moved.files > 0 && plural(moved.files, "file", "files"),
        ].filter((part): part is string => part !== false);
  return (
    <>
      {names(item.assets ?? [], "Missing files")}
      {engramsVisible && names(item.targets ?? [], "Engram attachments")}
      {item.asset !== null && (
        <p className="mt-1">
          <a
            href={reflectionAssetUrl(domain, item.asset)}
            target="_blank"
            rel="noreferrer"
            className={LINK}
          >
            {item.asset}
          </a>
        </p>
      )}
      {moved !== null && (
        <>
          <p className="mt-1">
            {counts.length === 0
              ? "This instance took over the domain."
              : `This instance took over the domain. Only on ${moved.previousHost ?? "the previous instance"}: ${joinWords(counts)}.`}
          </p>
          <p className="mt-1">
            {`Copy the domain folder over and sync. If the files are gone for good, run crystalline sync --domain ${domain} --accept-disk.`}
          </p>
        </>
      )}
    </>
  );
}
