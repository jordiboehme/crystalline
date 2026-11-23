/**
 * The column beside a Reflection page: what the page is beside its text.
 * The engram page's details column, for a page (Jordi 2026-11-23): one
 * column beside the page on wide screens, under it on narrow ones, quiet
 * labels over values and hairline rules between the sections.
 *
 * Its first section is the page settings (design 4): the page's freshness,
 * the page it follows, its own pact file, and its life (retire, restore,
 * delete). A curator changes them here; everybody else reads them. Later
 * sections (tags, links) go under it.
 *
 * Every refusal is the server's: a freshness under 1 hour, a `follows` that
 * closes a cycle, a page that is no longer retired. Its sentence is shown as
 * it comes. A setting is sent only when it changed, and a cleared one as an
 * empty value (the route keeps a key it is not sent).
 *
 * The retired banner over a retired page carries the same restore and delete
 * ({@link RetiredBanner}), so a curator who meets the banner first need not
 * look for the column.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactElement, ReactNode } from "react";
import { useId, useState } from "react";
import { Link, useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import type { TreeNode } from "../api/reflection";
import {
  fetchReflectionTree,
  reflectionTreeKey,
  saveReflection,
} from "../api/reflection";
import { invalidateReflection } from "../api/reflectionDomains";
import type { PageView } from "../api/reflectionPage";
import {
  restoreReflection,
  retireReflection,
  setReflectionSettings,
  settingsChanges,
} from "../api/reflectionPage";
import { reflectionEditRoute, reflectionPageRoute } from "../paths";
import { DestructiveAction } from "./DestructiveAction";
import {
  BUTTON,
  CHROME_LINK,
  FIELD,
  FOCUS_RING,
  RAIL_LABEL,
} from "./primitives";

/** The freshness presets the panel offers, as the server reads them. */
const PRESETS: readonly { value: string; label: string }[] = [
  { value: "1h", label: "1 hour" },
  { value: "6h", label: "6 hours" },
  { value: "1d", label: "1 day" },
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
];
const CUSTOM = "custom";

/** Said beside the custom freshness: `m` is months, as in build_context. */
export const CUSTOM_FRESHNESS_HELP =
  "A number and a unit: h hours, d days, w weeks, m months, y years. m means months, not minutes. A bare number is days. At least 1 hour.";

const STEP = `rounded underline-offset-[3px] ${CHROME_LINK} ${FOCUS_RING}`;

export interface PageDetailsProps {
  domain: string;
  path: string;
  title: string;
  page: PageView;
  /** Whether this person curates the Reflection and may change anything. */
  curator: boolean;
  /** Why nothing can be changed here (read only, a locked domain); undefined when it can. */
  lockedReason?: string | undefined;
  /** Delete the page; the page screen owns that mutation and where it leads. */
  onDelete: () => void;
  deleting: boolean;
}

export function ReflectionPageDetails(props: PageDetailsProps): ReactElement {
  return (
    <aside
      aria-label="Page details"
      className="flex flex-col divide-y divide-hair text-sm print:hidden [&>*]:py-4 [&>*:first-child]:pt-0"
    >
      <PageSettings {...props} />
    </aside>
  );
}

function PageSettings({
  domain,
  path,
  title,
  page,
  curator,
  lockedReason,
  onDelete,
  deleting,
}: PageDetailsProps): ReactElement {
  const changes = curator && lockedReason === undefined;
  return (
    <section aria-label="Page settings" className="flex flex-col gap-3">
      <h2 className={RAIL_LABEL}>Page settings</h2>
      {changes && page.retired === null ? (
        <SettingsForm
          // A fresh form for each stored state: a save the server took
          // comes back as new settings, and the form starts from those.
          key={`${page.freshness ?? ""}\u0000${page.follows ?? ""}`}
          domain={domain}
          path={path}
          page={page}
        />
      ) : (
        <dl className="flex flex-col">
          {page.owner === null && (
            <Row label="Freshness">{page.freshness ?? "None"}</Row>
          )}
          <Row label="Follows">
            {page.follows === null ? (
              "Not set"
            ) : (
              <Link
                to={reflectionPageRoute(domain, page.follows)}
                className={STEP}
              >
                {page.chain.at(-1)?.path === page.follows
                  ? page.chain.at(-1)?.title
                  : page.follows}
              </Link>
            )}
          </Row>
          {page.status !== null && <Row label="Status">{page.status}</Row>}
        </dl>
      )}
      <Findings page={page} />
      <PactRow domain={domain} title={title} page={page} mayCreate={changes} />
      {changes && (
        <PageLifecycle
          domain={domain}
          path={path}
          page={page}
          onDelete={onDelete}
          deleting={deleting}
        />
      )}
    </section>
  );
}

function Row({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}): ReactElement {
  return (
    <div className="grid grid-cols-[5.5rem_minmax(0,1fr)] items-baseline gap-x-3 py-1.5">
      <dt className="text-muted">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </div>
  );
}

/** The pages of the domain a page may follow: every page but itself and the retired. */
function followable(
  nodes: TreeNode[],
  self: string,
): { path: string; title: string }[] {
  return nodes.flatMap((node) =>
    node.kind === "folder"
      ? followable(node.children, self)
      : node.path === self || node.retired
        ? []
        : [{ path: node.path, title: node.title }],
  );
}

/** The freshness the page's file holds, even one the server ignores. */
function storedFreshness(page: PageView, path: string): string | null {
  return (
    page.freshness ??
    page.findings.find((f) => f.key === "freshness" && f.page === path)
      ?.value ??
    null
  );
}

function SettingsForm({
  domain,
  path,
  page,
}: {
  domain: string;
  path: string;
  page: PageView;
}): ReactElement {
  const queryClient = useQueryClient();
  const freshnessId = useId();
  const customId = useId();
  const followsId = useId();
  const owned = page.owner !== null;
  const stored = storedFreshness(page, path);
  const [choice, setChoice] = useState(() =>
    stored === null
      ? ""
      : PRESETS.some((p) => p.value === stored)
        ? stored
        : CUSTOM,
  );
  const [custom, setCustom] = useState(choice === CUSTOM ? (stored ?? "") : "");
  const [follows, setFollows] = useState(page.follows ?? "");
  const [saved, setSaved] = useState(false);
  const tree = useQuery({
    queryKey: reflectionTreeKey(domain),
    queryFn: () => fetchReflectionTree(domain),
  });
  const options = followable(tree.data?.nodes ?? [], path);
  if (page.follows !== null && !options.some((o) => o.path === page.follows)) {
    options.unshift({ path: page.follows, title: page.follows });
  }
  const wanted = {
    ...(owned ? {} : { freshness: choice === CUSTOM ? custom : choice }),
    follows,
  };
  const pending = settingsChanges(
    { freshness: stored, follows: page.follows },
    wanted,
  );
  const dirty = Object.keys(pending).length > 0;
  const save = useMutation({
    mutationFn: () => setReflectionSettings(domain, path, pending),
    onMutate: () => {
      setSaved(false);
    },
    onSuccess: () => {
      setSaved(true);
      invalidateReflection(queryClient, domain);
    },
  });
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (dirty && !save.isPending) {
          save.mutate();
        }
      }}
    >
      {owned ? (
        <p className="text-muted">
          This page is part of another page and is checked with it.
        </p>
      ) : (
        <div className="flex flex-col gap-1">
          <label htmlFor={freshnessId} className="text-muted">
            Freshness
          </label>
          <select
            id={freshnessId}
            value={choice}
            onChange={(event) => {
              setChoice(event.target.value);
            }}
            className={FIELD}
          >
            <option value="">None</option>
            {PRESETS.map((p) => (
              <option key={p.value} value={p.value}>
                {p.label}
              </option>
            ))}
            <option value={CUSTOM}>Custom</option>
          </select>
          {choice === CUSTOM && (
            <>
              <label htmlFor={customId} className="sr-only">
                Custom freshness
              </label>
              <input
                id={customId}
                value={custom}
                placeholder="12h"
                autoComplete="off"
                aria-describedby={`${customId}-help`}
                onChange={(event) => {
                  setCustom(event.target.value);
                }}
                className={FIELD}
              />
              <p
                id={`${customId}-help`}
                className="text-caption text-slate-500 dark:text-slate-400"
              >
                {CUSTOM_FRESHNESS_HELP}
              </p>
            </>
          )}
        </div>
      )}
      <div className="flex flex-col gap-1">
        <label htmlFor={followsId} className="text-muted">
          Follows
        </label>
        <select
          id={followsId}
          value={follows}
          onChange={(event) => {
            setFollows(event.target.value);
          }}
          className={FIELD}
        >
          <option value="">Not set</option>
          {options.map((o) => (
            <option key={o.path} value={o.path}>
              {o.title}
            </option>
          ))}
        </select>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="submit"
          aria-disabled={!dirty || save.isPending}
          className={`${BUTTON.secondary} aria-disabled:cursor-default aria-disabled:opacity-50`}
        >
          Save settings
        </button>
        <span role="status" className="text-caption text-muted">
          {saved && !dirty ? "Saved" : ""}
        </span>
      </div>
      {save.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(save.error)}
        </p>
      )}
    </form>
  );
}

/** What the server ignores in the page's settings, and why. */
function Findings({ page }: { page: PageView }): ReactElement | null {
  if (page.findings.length === 0) {
    return null;
  }
  return (
    <ul className="flex flex-col gap-1 text-caption text-amber-800 dark:text-amber-300">
      {page.findings.map((f) => (
        <li key={`${f.page}\u0000${f.key}`}>
          {`${f.key}: ${f.value}. ${f.reason}`}
        </li>
      ))}
    </ul>
  );
}

/** The page's own pact file: open it, or create it for a curator. */
function PactRow({
  domain,
  title,
  page,
  mayCreate,
}: {
  domain: string;
  title: string;
  page: PageView;
  mayCreate: boolean;
}): ReactElement {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const create = useMutation({
    mutationFn: () =>
      saveReflection(domain, page.pact.path, `# Pact for ${title}\n`, {
        create: true,
      }),
    onSuccess: () => {
      invalidateReflection(queryClient, domain);
      void navigate(reflectionEditRoute(domain, page.pact.path));
    },
  });
  return (
    <div className="flex flex-col gap-1">
      <span className="text-muted">Page pact</span>
      {page.pact.exists ? (
        <Link
          to={reflectionPageRoute(domain, page.pact.path)}
          className={`self-start ${STEP}`}
        >
          Open the page pact
        </Link>
      ) : mayCreate ? (
        <button
          type="button"
          disabled={create.isPending}
          onClick={() => {
            create.mutate();
          }}
          className={`self-start ${BUTTON.secondary}`}
        >
          Create the page pact
        </button>
      ) : (
        <span>None</span>
      )}
      {create.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(create.error)}
        </p>
      )}
    </div>
  );
}

/**
 * Retire a page, or restore a retired one (on its own, or back under the
 * page it was part of while that page is still there), and delete it with
 * the app's two-step confirm.
 */
export function PageLifecycle({
  domain,
  path,
  page,
  onDelete,
  deleting,
}: {
  domain: string;
  path: string;
  page: PageView;
  onDelete: () => void;
  deleting: boolean;
}): ReactElement {
  const queryClient = useQueryClient();
  const change = useMutation({
    mutationFn: (
      act: { kind: "retire" } | { kind: "restore"; owner?: string },
    ) =>
      act.kind === "retire"
        ? retireReflection(domain, path)
        : restoreReflection(domain, path, act.owner),
    onSuccess: () => {
      invalidateReflection(queryClient, domain);
    },
  });
  const owner = page.retired?.wasOwnedBy ?? null;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        {page.retired === null ? (
          <button
            type="button"
            disabled={change.isPending}
            onClick={() => {
              change.mutate({ kind: "retire" });
            }}
            className={BUTTON.secondary}
          >
            Retire
          </button>
        ) : (
          <>
            <button
              type="button"
              disabled={change.isPending}
              onClick={() => {
                change.mutate({ kind: "restore" });
              }}
              className={BUTTON.secondary}
            >
              Restore
            </button>
            {owner?.exists === true && (
              <button
                type="button"
                disabled={change.isPending}
                onClick={() => {
                  change.mutate({ kind: "restore", owner: owner.path });
                }}
                className={BUTTON.secondary}
              >
                {`Restore under ${owner.title}`}
              </button>
            )}
          </>
        )}
        <DestructiveAction
          label="Delete"
          confirmLabel="Confirm delete"
          pending={deleting}
          onConfirm={onDelete}
        />
      </div>
      {change.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(change.error)}
        </p>
      )}
    </div>
  );
}

/**
 * Over a retired page: "Retired: no longer part of Architecture overview",
 * or just "Retired" for a page that was on its own, with restore and delete
 * for a curator.
 */
export function RetiredBanner({
  domain,
  path,
  page,
  curator,
  onDelete,
  deleting,
}: {
  domain: string;
  path: string;
  page: PageView;
  curator: boolean;
  onDelete: () => void;
  deleting: boolean;
}): ReactElement | null {
  if (page.retired === null) {
    return null;
  }
  const owner = page.retired.wasOwnedBy;
  return (
    <aside
      role="note"
      aria-label="Retired"
      className="flex flex-col gap-2 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 print:hidden dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100"
    >
      <p>
        {owner === null
          ? "Retired"
          : `Retired: no longer part of ${owner.title}`}
      </p>
      {curator && (
        <PageLifecycle
          domain={domain}
          path={path}
          page={page}
          onDelete={onDelete}
          deleting={deleting}
        />
      )}
    </aside>
  );
}
