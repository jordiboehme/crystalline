/**
 * A domain's Reflection: where its pages are for people.
 *
 * The page itself is short on purpose. The tree lives in the sidebar, the
 * way the engram tree does, so this screen holds what the tree cannot: the
 * pact the pages are written to, the way to the review screen with the open
 * thread count, search, and - for a contributor, who has no domain page -
 * the way out of the domain.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { BookOpen, ChevronDown, Inbox, Search } from "lucide-react";
import { useId, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";

import { lastRunSentence, useAutomation } from "../api/automation";
import { ApiProblem, problemDetail } from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { removeMember } from "../api/members";
import type { LoopState } from "../api/automation";
import type {
  ReflectionHomeSummary,
  ReflectionSearchMode,
  ReflectionTree,
  TreeNode,
} from "../api/reflection";
import {
  REFLECTION_SEARCH_MODES,
  fetchReflectionTree,
  openThreadCount,
  readReflectionSearchMode,
  reflectionSearchKey,
  reflectionTreeKey,
  searchReflections,
  useReflectionHomeSummary,
} from "../api/reflection";
import { REFLECTION_DOMAINS_KEY, atLeast } from "../api/reflectionDomains";
import { useAuth } from "../auth/AuthContext";
import {
  DestructiveAction,
  READ_ONLY_REASON,
} from "../components/DestructiveAction";
import { GithubLinkHint } from "../components/GithubLinkHint";
import { useUnlinkedFromGithub } from "../githubLink";
import { useReflectionStatus } from "../components/ReflectionModeSwitch";
import { ReflectionStateNotice } from "../components/ReflectionStateNotice";
import { Avatar } from "../components/ReflectionThreads";
import { BUTTON, Chip, FOCUS_RING, RAIL_LABEL } from "../components/primitives";
import { WritePact } from "../components/WritePact";
import { Loading } from "../components/Loading";
import { plural, relativeTime } from "../format";
import {
  reflectionPageRoute,
  reflectionReviewRoute,
  reflectionSearchRoute,
} from "../paths";
import { useHeadingFocusRequest } from "../focusHeading";
import {
  stripHtmlComments,
  withoutHtmlComments,
} from "../markdown/htmlComments";
import { stripIdLines } from "../reflectionText";

/** The refusal face, the one `DomainPoliciesCard` announces a problem in. */
const ALERT =
  "rounded bg-red-50 px-2 py-1 text-sm text-red-800 dark:bg-red-950 dark:text-red-200";

const LINK =
  "text-link underline decoration-(--link-line) decoration-1 underline-offset-[3px] hover:no-underline";

export default function ReflectionHome(): ReactElement {
  const { domain = "" } = useParams();
  const status = useReflectionStatus(domain);
  useHeadingFocusRequest(!status.isPending);
  if (status.isPending) {
    return (
      <Loading
        label="Loading the Reflection"
        subject={`${domain} reflection`}
      />
    );
  }
  if (status.error) {
    // Only a 404 is a wrong address; any other failure is the server's own
    // words, never dressed up as a missing domain.
    return (
      <div className="flex flex-col items-start gap-3">
        <h1 className="text-display">
          {isMissing(status.error) ? "Domain not found" : domain}
        </h1>
        {isMissing(status.error) ? (
          <p className="text-sm">{problemDetail(status.error)}</p>
        ) : (
          <p role="alert" className={ALERT}>
            {problemDetail(status.error)}
          </p>
        )}
      </div>
    );
  }
  const s = status.data;
  return (
    <div className="flex max-w-3xl flex-col gap-2">
      <p className={RAIL_LABEL}>Reflection</p>
      <h1 className="text-display">{domain}</h1>
      <p className="text-[17px] leading-relaxed text-ink-3">
        Pages that explain this domain to people. Agents keep them in step with
        the engrams; you read, comment and edit.
      </p>
      <ReflectionStateNotice status={s} />
      {s.state === "on" && (
        <OnHome domain={domain} mayWritePact={atLeast(s.right, "curate")} />
      )}
      <UnlinkedReflectionHint
        show={s.governance === "github" && s.private && !s.engramsVisible}
      />
      {/* In every state, not only while it is on: a member must be able to
          leave a domain whose Reflection is pending or paused too. */}
      {s.member && !s.engramsVisible && <LeaveDomain domain={domain} />}
    </div>
  );
}

/** The unlinked hint on a private GitHub domain's Reflection. */
function UnlinkedReflectionHint({
  show,
}: {
  show: boolean;
}): ReactElement | null {
  const unlinked = useUnlinkedFromGithub(show);
  return unlinked ? <GithubLinkHint /> : null;
}

function OnHome({
  domain,
  mayWritePact,
}: {
  domain: string;
  mayWritePact: boolean;
}): ReactElement {
  const tree = useQuery({
    queryKey: reflectionTreeKey(domain),
    queryFn: () => fetchReflectionTree(domain),
  });
  const summary = useReflectionHomeSummary(domain).data ?? null;
  const reflect = useAutomation(domain, { loop: "reflect" }).data?.reflect;
  return (
    <>
      <SearchSection domain={domain} />
      {tree.error && (
        <p role="alert" className={ALERT}>
          {problemDetail(tree.error)}
        </p>
      )}
      {tree.data && (
        <div className="mt-7 grid gap-3.5 sm:grid-cols-2">
          <PactCard
            domain={domain}
            tree={tree.data}
            mayWritePact={mayWritePact}
          />
          <ReviewCard
            domain={domain}
            threads={openThreadCount(tree.data)}
            summary={summary}
            reflect={reflect}
          />
        </div>
      )}
      {summary && <OpenThreads domain={domain} summary={summary} />}
      {tree.data && <PagesList domain={domain} tree={tree.data} />}
    </>
  );
}

const CARD = "rounded-xl border border-hair bg-raised px-[18px] py-4";
const CARD_TITLE = "flex items-center gap-2 text-[15px] font-semibold text-ink";
const CARD_TEXT = "mt-1 text-[13.5px] leading-normal text-muted";

function PactCard({
  domain,
  tree,
  mayWritePact,
}: {
  domain: string;
  tree: ReflectionTree;
  mayWritePact: boolean;
}): ReactElement {
  const icon = (
    <BookOpen
      aria-hidden="true"
      size={16}
      strokeWidth={1.75}
      className="text-accent-500"
    />
  );
  if (tree.pact.exists) {
    return (
      <section className={CARD} aria-label="Pact">
        <h2 className={CARD_TITLE}>
          {icon}
          <Link
            to={reflectionPageRoute(domain, tree.pact.path)}
            className={LINK}
          >
            {tree.pact.title}
          </Link>
        </h2>
        <p className={CARD_TEXT}>
          The pact: tone, structure and what goes where. Read it before your
          first page.
        </p>
      </section>
    );
  }
  return (
    <section className={CARD} aria-label="Pact">
      <h2 className={CARD_TITLE}>
        {icon}
        {`How we write about ${domain}`}
      </h2>
      {mayWritePact ? (
        <>
          <p className={CARD_TEXT}>
            The pact says how pages are written. It does not exist yet.
          </p>
          <div className="mt-3">
            <WritePact domain={domain} />
          </div>
        </>
      ) : (
        <p className={CARD_TEXT}>No pact is written yet.</p>
      )}
    </section>
  );
}

/** What waits, in one sentence: threads by who must answer, pages to update. */
function reviewSentence(summary: ReflectionHomeSummary): string {
  const agent = summary.waitingForAgent;
  const people = summary.waitingForPeople;
  const updates = summary.pagesToUpdate;
  const parts = [
    agent > 0 &&
      `${plural(agent, "thread waits", "threads wait")} for the agent.`,
    people > 0 &&
      `${plural(people, "thread waits", "threads wait")} for people.`,
    updates > 0 && `${plural(updates, "page needs", "pages need")} an update.`,
  ].filter((part): part is string => part !== false);
  return parts.length > 0 ? parts.join(" ") : "Nothing waits for review.";
}

function ReviewCard({
  domain,
  threads,
  summary,
  reflect,
}: {
  domain: string;
  threads: number;
  summary: ReflectionHomeSummary | null;
  reflect: LoopState | null | undefined;
}): ReactElement {
  const run = reflect?.last_run ?? null;
  return (
    <section className={CARD} aria-label="Review">
      <h2 className={CARD_TITLE}>
        <Inbox
          aria-hidden="true"
          size={16}
          strokeWidth={1.75}
          className="text-accent-500"
        />
        <Link to={reflectionReviewRoute(domain)} className={LINK}>
          Review
        </Link>
        {threads > 0 && (
          <Chip variant="accent">
            <span aria-hidden="true">{threads}</span>
            <span className="sr-only">
              {plural(threads, "open thread", "open threads")}
            </span>
          </Chip>
        )}
      </h2>
      <p className={CARD_TEXT}>
        {summary === null
          ? plural(threads, "open thread", "open threads")
          : reviewSentence(summary)}
      </p>
      {run !== null && (
        <p className={CARD_TEXT}>
          {lastRunSentence(
            "reflected",
            run,
            relativeTime(run.ended_at, new Date()),
          )}
          {reflect?.left_for_person != null && reflect.left_for_person > 0 && (
            <>
              {" "}
              <Link to={reflectionReviewRoute(domain)} className={LINK}>
                {`${String(reflect.left_for_person)} waiting for a person`}
              </Link>
            </>
          )}
        </p>
      )}
    </section>
  );
}

const SECTION_HEAD = "mt-10 mb-2 flex items-baseline gap-2.5";
const ROWS = "border-t border-hair";
const ROW =
  "grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-0.5 border-b border-hair px-0.5 py-2.5";
const ROW_SIDE =
  "col-start-2 row-span-2 row-start-1 flex items-center gap-2.5 text-[12.5px] text-muted";

function OpenThreads({
  domain,
  summary,
}: {
  domain: string;
  summary: ReflectionHomeSummary;
}): ReactElement | null {
  const groups = summary.openByPage;
  if (groups.length === 0) {
    return null;
  }
  return (
    <section aria-labelledby="home-threads">
      <div className={SECTION_HEAD}>
        <h2 id="home-threads" className="text-[21px] font-semibold">
          Open threads
        </h2>
      </div>
      <ul aria-label="Pages with open threads" className={ROWS}>
        {groups.map((group) => {
          const people = new Map<string, boolean>();
          for (const thread of group.threads) {
            for (const person of thread.people) {
              people.set(person.name, person.agent);
            }
          }
          const quote = group.threads[0]?.quote ?? null;
          const who = [...people]
            .map(([name, agent]) => (agent ? `${name} (agent)` : name))
            .join(", ");
          return (
            <li key={group.path} className={ROW}>
              <Link
                to={reflectionPageRoute(domain, group.path)}
                className={`text-[15px] font-medium ${LINK}`}
              >
                {group.title}
              </Link>
              <span className={ROW_SIDE}>
                <span className="flex -space-x-1.5">
                  {[...people].map(([name, agent]) => (
                    <Avatar key={name} name={name} agent={agent} />
                  ))}
                </span>
                {`${String(group.threads.length)} open`}
              </span>
              <span className="truncate text-[13px] text-muted">
                {[quote !== null && quote !== "" ? `"${quote}"` : "", who]
                  .filter((part) => part !== "")
                  .join(" · ")}
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

interface PageRow {
  path: string;
  title: string;
  folder: string;
  sections: number;
  openThreads: number;
}

function pageRows(nodes: TreeNode[], folder = ""): PageRow[] {
  return nodes.flatMap((node) =>
    node.kind === "folder"
      ? pageRows(node.children, node.path)
      : [
          {
            path: node.path,
            title: node.title,
            folder,
            sections: node.sections,
            openThreads: node.openThreads,
          },
        ],
  );
}

function PagesList({
  domain,
  tree,
}: {
  domain: string;
  tree: ReflectionTree;
}): ReactElement {
  const rows = pageRows(tree.nodes);
  return (
    <section aria-labelledby="home-pages">
      <div className={SECTION_HEAD}>
        <h2 id="home-pages" className="text-[21px] font-semibold">
          Pages
        </h2>
        <span className="text-[13px] text-muted">{rows.length}</span>
      </div>
      {rows.length === 0 ? (
        <p className="text-sm text-muted">No page is written yet.</p>
      ) : (
        <ul aria-label="All pages" className={ROWS}>
          {rows.map((row) => (
            <li key={row.path} className={ROW}>
              <Link
                to={reflectionPageRoute(domain, row.path)}
                className={`text-[15px] font-medium ${LINK}`}
              >
                {row.title}
              </Link>
              <span className={ROW_SIDE}>
                {[
                  plural(row.sections, "section", "sections"),
                  row.openThreads > 0
                    ? plural(row.openThreads, "open thread", "open threads")
                    : "",
                ]
                  .filter((part) => part !== "")
                  .join(" · ")}
              </span>
              <span className="text-[13px] text-muted">{row.folder}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * What ran, in the engram search's manner (`Search.tsx`'s status line): the
 * mode that ran is what explains an answer, and a fallback says why.
 */
function searchStatus(
  ran: ReflectionSearchMode | null,
  asked: ReflectionSearchMode,
  total: number,
): string | null {
  if (ran === null) {
    return null;
  }
  if (ran !== asked) {
    return `Asked for ${asked}, ran as ${ran}: the pages are not embedded yet.`;
  }
  return `${plural(total, "hit", "hits")}, ranked by ${ran}.`;
}

function SearchSection({ domain }: { domain: string }): ReactElement {
  const [params] = useSearchParams();
  const navigate = useNavigate();
  // The query, the mode and the page live in the address, so a result page
  // is a link somebody can send (as on the engram search screen).
  const asked = params.get("q") ?? "";
  const mode = readReflectionSearchMode(params.get("search_type"));
  const page = Math.max(1, Number(params.get("page") ?? "1") || 1);
  const [draft, setDraft] = useState(asked);
  const [picked, setPicked] = useState<ReflectionSearchMode>(mode);
  // The field and the select follow the address when it changes under them
  // (back, forward, a link to this screen), adjusted while rendering rather
  // than in an effect so no frame shows the old words.
  const [shown, setShown] = useState({ asked, mode });
  if (shown.asked !== asked || shown.mode !== mode) {
    setShown({ asked, mode });
    setDraft(asked);
    setPicked(mode);
  }
  const fieldId = useId();
  const modeId = useId();
  const search = useQuery({
    queryKey: reflectionSearchKey(domain, asked, mode, page),
    queryFn: () => searchReflections(domain, { query: asked, mode, page }),
    enabled: asked.trim() !== "",
  });
  const go = (
    query: string,
    nextMode: ReflectionSearchMode,
    nextPage: number,
  ) => {
    void navigate(reflectionSearchRoute(domain, query, nextMode, nextPage));
  };
  const answer = search.data;
  const hits = answer?.hits;
  const lastPage = answer
    ? Math.max(1, Math.ceil(answer.total / Math.max(answer.limit, 1)))
    : 1;
  // An empty page says one thing: a count beside "No page matches." would
  // contradict it on a page past the last one.
  const status =
    hits !== undefined && hits.length > 0 && answer
      ? searchStatus(answer.mode, mode, answer.total)
      : null;
  return (
    <div className="mt-5 flex flex-col gap-3">
      <form
        role="search"
        aria-label="Search the Reflection"
        className="flex h-[46px] items-center gap-2.5 rounded-xl border border-hair-strong bg-raised pr-1.5 pl-3.5 shadow-xs focus-within:ring-2 focus-within:ring-accent-600 dark:focus-within:ring-accent-400"
        onSubmit={(event) => {
          event.preventDefault();
          if (draft.trim() !== "") {
            go(draft.trim(), picked, 1);
          }
        }}
      >
        <Search
          aria-hidden="true"
          size={18}
          strokeWidth={1.75}
          className="shrink-0 text-faint"
        />
        <label htmlFor={fieldId} className="sr-only">
          Search the Reflection
        </label>
        <input
          id={fieldId}
          type="search"
          value={draft}
          placeholder="Search the Reflection"
          onChange={(event) => {
            setDraft(event.target.value);
          }}
          className="min-w-0 flex-1 bg-transparent text-[15px] text-ink outline-none placeholder:text-muted"
        />
        <span className="relative inline-flex shrink-0 items-center text-[12.5px] text-muted">
          <label htmlFor={modeId} className="sr-only">
            Mode
          </label>
          <select
            id={modeId}
            value={picked}
            onChange={(event) => {
              const next = readReflectionSearchMode(event.target.value);
              setPicked(next);
              if (asked.trim() !== "") {
                go(asked, next, 1);
              }
            }}
            className={`h-8 cursor-pointer appearance-none rounded bg-transparent pr-5 pl-2 text-[12.5px] text-muted hover:bg-sunken ${FOCUS_RING}`}
          >
            {REFLECTION_SEARCH_MODES.map((value) => (
              <option key={value} value={value}>
                {value.charAt(0).toUpperCase() + value.slice(1)}
              </option>
            ))}
          </select>
          <ChevronDown
            aria-hidden="true"
            size={14}
            className="pointer-events-none absolute right-1"
          />
        </span>
        <button
          type="submit"
          aria-label="Search"
          className={`inline-grid size-8 shrink-0 place-items-center rounded-lg bg-accent-700 text-white hover:bg-accent-800 dark:bg-accent-400 dark:text-accent-950 dark:hover:bg-accent-300 ${FOCUS_RING}`}
        >
          <Search aria-hidden="true" size={16} strokeWidth={2} />
        </button>
      </form>
      {search.error && (
        <p className="text-sm text-red-800 dark:text-red-200">
          {problemDetail(search.error)}
        </p>
      )}
      {status !== null && (
        <p className="text-caption text-slate-600 dark:text-slate-300">
          {status}
        </p>
      )}
      {hits?.length === 0 && (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          No page matches.
        </p>
      )}
      {hits && hits.length > 0 && (
        <ul className="flex flex-col gap-3">
          {hits.map((hit, index) => (
            // The rank keeps two hits apart that share a path and have no
            // section id yet; the list is redrawn whole on every answer.
            <li key={`${String(index)}:${hit.path}#${hit.section ?? ""}`}>
              <Link
                to={`${reflectionPageRoute(domain, hit.path)}${hit.section !== null ? `#s-${hit.section}` : ""}`}
                className={LINK}
              >
                {withoutHtmlComments(hit.heading).trim() !== ""
                  ? `${hit.title}: ${withoutHtmlComments(hit.heading).trim()}`
                  : hit.title}
              </Link>
              <p className="text-caption text-slate-600 dark:text-slate-300">
                {stripHtmlComments(stripIdLines(hit.snippet))}
              </p>
            </li>
          ))}
        </ul>
      )}
      {answer && (lastPage > 1 || page > 1) && (
        <div className="flex gap-2">
          {page > 1 && (
            <button
              type="button"
              onClick={() => {
                // From past the end, back to the last page there is.
                go(asked, mode, Math.min(page - 1, lastPage));
              }}
              className={BUTTON.secondary}
            >
              Previous page
            </button>
          )}
          {page < lastPage && (
            <button
              type="button"
              onClick={() => {
                go(asked, mode, page + 1);
              }}
              className={BUTTON.secondary}
            >
              Next page
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * The way out for a member with knowledge `none`. It has no domain page (it
 * reads no engram), so the members card is out of reach. Drawn only when the
 * status says `member` (G5): knowledge `none` set on the instance for every
 * shared domain holds no membership to leave. The leave call is the members
 * route's own, gated on membership by core Task 17.
 */
function LeaveDomain({ domain }: { domain: string }): ReactElement {
  const { user, capabilities } = useAuth();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const leave = useMutation({
    mutationFn: () => removeMember(domain, user?.name ?? ""),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: REFLECTION_DOMAINS_KEY });
      void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
      void navigate("/");
    },
  });
  return (
    <section
      aria-labelledby="reflection-leave"
      className="flex flex-col items-start gap-2"
    >
      <h2 id="reflection-leave" className="text-section">
        Membership
      </h2>
      <DestructiveAction
        label="Leave this domain"
        confirmLabel="Confirm leave"
        pending={leave.isPending}
        disabledReason={capabilities.readOnly ? READ_ONLY_REASON : undefined}
        onConfirm={() => {
          leave.mutate();
        }}
      />
      {leave.error && (
        <p role="alert" className="text-sm text-red-800 dark:text-red-200">
          {problemDetail(leave.error)}
        </p>
      )}
    </section>
  );
}

/** Whether this failure is the server saying there is nothing at that address. */
function isMissing(error: unknown): boolean {
  return error instanceof ApiProblem && error.status === 404;
}
