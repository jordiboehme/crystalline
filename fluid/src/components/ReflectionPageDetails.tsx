/**
 * The column beside a Reflection page: what the page is beside its text.
 * The engram page's details column, for a page (Jordi 2026-11-23): one
 * column beside the page on wide screens, under it on narrow ones, quiet
 * labels over values and hairline rules between the sections.
 *
 * Its first section is the page settings (design 4), a read-only view for
 * everybody: the page's freshness, the page it follows ("No page, only
 * PACT.md" when it follows none), its status when set, and its own pact
 * file (a link, or "None" with a Create action for a curator). A curator on
 * a page that is not retired, on an instance and a domain that take writes,
 * gets an Edit action beside the heading: it swaps the view for the form
 * with Save and Cancel, its first field focused. Escape cancels, but not
 * one that ends a composition; an Escape that closes a native select's
 * list mostly never reaches the page, so it is not told apart. After a
 * save the server took, or a cancel, the view comes back and the focus
 * returns to Edit; a save is also said as "Saved" in a status beside the
 * heading, until the next Edit. A retire or a lock closes the form, and it
 * stays closed after a restore. Later sections (tags, links) go under it.
 *
 * Every refusal is the server's: a freshness under 1 hour, a `follows` that
 * closes a cycle. Its sentence is shown in the form, which stays open. A
 * setting is sent only when the person changed it from what the page held
 * when the form opened, so a save never undoes what another curator saved
 * in the meantime. A cleared one is sent as an empty value (the route
 * keeps a key it is not sent).
 *
 * The column holds no lifecycle buttons: Retire and Delete are in the
 * page's More actions menu, and a retired page shows the retired banner
 * ({@link RetiredBanner}) with Restore and Delete.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { KeyboardEvent, ReactElement, ReactNode } from "react";
import { useEffect, useId, useRef, useState } from "react";
import { Link, useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import type { ReflectionDoc, TreeNode } from "../api/reflection";
import {
  fetchReflectionTree,
  reflectionDocKey,
  reflectionTreeKey,
  saveReflection,
} from "../api/reflection";
import { invalidateReflection } from "../api/reflectionDomains";
import type { PageView } from "../api/reflectionPage";
import {
  restoreReflection,
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

/** A small text action beside a heading or a value: Edit, Create. */
const SMALL_ACTION = `rounded text-caption underline underline-offset-[3px] disabled:opacity-50 ${CHROME_LINK} ${FOCUS_RING}`;

/** What the view and the form say for a page that follows no page. */
const NO_FOLLOWS = "No page, only PACT.md";

const UNITS: Readonly<Record<string, readonly [string, string]>> = {
  h: ["hour", "hours"],
  d: ["day", "days"],
  w: ["week", "weeks"],
  m: ["month", "months"],
  y: ["year", "years"],
};

/**
 * A freshness in words, as the form's presets say them: "1 day", "6 hours",
 * "3 months" (`m` is months), a bare number in days. Anything else as
 * written, a unit in capitals too: the server reads the units in lower case
 * only, so "3M" is no freshness and must not read as one.
 */
function freshnessWords(value: string): string {
  const read = /^\s*(\d+)\s*([hdwmy])?\s*$/.exec(value);
  const unit = UNITS[read?.[2] ?? "d"];
  if (read === null || unit === undefined) {
    return value;
  }
  const count = Number(read[1]);
  return `${String(count)} ${count === 1 ? unit[0] : unit[1]}`;
}

export interface PageDetailsProps {
  domain: string;
  path: string;
  title: string;
  page: PageView;
  /** Whether this person curates the Reflection and may change anything. */
  curator: boolean;
  /** Why nothing can be changed here (read only, a locked domain); undefined when it can. */
  lockedReason?: string | undefined;
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
}: PageDetailsProps): ReactElement {
  const changes = curator && lockedReason === undefined;
  const editable = changes && page.retired === null;
  const headingId = useId();
  const [editing, setEditing] = useState(false);
  // A retire or a lock ends the editing, so a restore does not open the
  // form again by itself.
  if (editing && !editable) {
    setEditing(false);
  }
  const [saved, setSaved] = useState(false);
  const editRef = useRef<HTMLButtonElement | null>(null);
  // Set when the form closes, so Edit takes the focus back once it shows.
  const refocus = useRef(false);
  useEffect(() => {
    if (!editing && refocus.current) {
      refocus.current = false;
      editRef.current?.focus();
    }
  }, [editing]);
  const close = (didSave: boolean): void => {
    refocus.current = true;
    setSaved(didSave);
    setEditing(false);
  };
  const open = editing && editable;
  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-3">
      <div className="flex items-baseline justify-between gap-2">
        <h2 id={headingId} className={RAIL_LABEL}>
          Page settings
        </h2>
        <div className="flex items-baseline gap-2">
          <span role="status" className="text-caption text-muted">
            {saved ? "Saved" : ""}
          </span>
          {editable && !open && (
            <button
              ref={editRef}
              type="button"
              aria-label="Edit page settings"
              onClick={() => {
                setSaved(false);
                setEditing(true);
              }}
              className={SMALL_ACTION}
            >
              Edit
            </button>
          )}
        </div>
      </div>
      {open ? (
        <SettingsForm domain={domain} path={path} page={page} onDone={close} />
      ) : (
        <dl className="flex flex-col">
          {page.owner === null && (
            <Row label="Freshness">
              {page.freshness === null
                ? "None"
                : freshnessWords(page.freshness)}
            </Row>
          )}
          <Row label="Follows">
            {page.follows === null ? (
              NO_FOLLOWS
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
          <Row label="Page pact">
            <PactValue
              domain={domain}
              title={title}
              page={page}
              mayCreate={changes}
            />
          </Row>
        </dl>
      )}
      <Findings page={page} />
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
  onDone,
}: {
  domain: string;
  path: string;
  page: PageView;
  /** Close the form: with true after a save the server took, with false on Cancel. */
  onDone: (saved: boolean) => void;
}): ReactElement {
  const queryClient = useQueryClient();
  const formRef = useRef<HTMLFormElement | null>(null);
  // Edit goes away when the form opens: its first field takes the focus.
  useEffect(() => {
    formRef.current?.querySelector<HTMLElement>("select, input")?.focus();
  }, []);
  const freshnessId = useId();
  const customId = useId();
  const followsId = useId();
  const owned = page.owner !== null;
  // The settings as they were when the form opened: what the person changes
  // is told apart from these, not from a page read again since.
  const [base] = useState(() => ({
    freshness: storedFreshness(page, path),
    follows: page.follows,
  }));
  const stored = base.freshness;
  const [choice, setChoice] = useState(() =>
    stored === null
      ? ""
      : PRESETS.some((p) => p.value === stored)
        ? stored
        : CUSTOM,
  );
  const [custom, setCustom] = useState(choice === CUSTOM ? (stored ?? "") : "");
  const [follows, setFollows] = useState(base.follows ?? "");
  const tree = useQuery({
    queryKey: reflectionTreeKey(domain),
    queryFn: () => fetchReflectionTree(domain),
  });
  const options = followable(tree.data?.nodes ?? [], path);
  const opened = base.follows;
  if (opened !== null && !options.some((o) => o.path === opened)) {
    options.unshift({ path: opened, title: opened });
  }
  // Custom with nothing typed yet changes nothing: only None clears.
  const keepFreshness = owned || (choice === CUSTOM && custom.trim() === "");
  const wanted = {
    ...(keepFreshness
      ? {}
      : { freshness: choice === CUSTOM ? custom : choice }),
    follows,
  };
  const pending = settingsChanges(base, wanted);
  const dirty = Object.keys(pending).length > 0;
  const save = useMutation({
    mutationFn: () => setReflectionSettings(domain, path, pending),
    onSuccess: (receipt) => {
      // The view shows the stored settings at once; the refetch below
      // brings the rest of the page (and its checksum) in line.
      if (receipt.page !== null) {
        const next = receipt.page;
        queryClient.setQueryData<ReflectionDoc>(
          reflectionDocKey(domain, path),
          (old) => (old === undefined ? old : { ...old, page: next }),
        );
      }
      invalidateReflection(queryClient, domain);
      onDone(true);
    },
  });
  return (
    <form
      ref={formRef}
      aria-label="Page settings"
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (dirty && !save.isPending) {
          save.mutate();
        }
      }}
      onKeyDown={(event: KeyboardEvent<HTMLFormElement>) => {
        if (
          event.key === "Escape" &&
          !event.nativeEvent.isComposing &&
          !event.defaultPrevented &&
          !save.isPending
        ) {
          event.preventDefault();
          event.stopPropagation();
          onDone(false);
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
          <option value="">{NO_FOLLOWS}</option>
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
          Save
        </button>
        <button
          type="button"
          disabled={save.isPending}
          onClick={() => {
            onDone(false);
          }}
          className={BUTTON.secondary}
        >
          Cancel
        </button>
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

/**
 * The page's own pact file: a link named by its file name, or "None" with
 * a Create action for a curator, which makes the file and opens it in the
 * editor.
 */
function PactValue({
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
  if (page.pact.exists) {
    return (
      <Link to={reflectionPageRoute(domain, page.pact.path)} className={STEP}>
        {page.pact.path.slice(page.pact.path.lastIndexOf("/") + 1)}
      </Link>
    );
  }
  return (
    <span className="flex flex-col gap-1">
      <span className="flex flex-wrap items-baseline gap-2">
        None
        {mayCreate && (
          <button
            type="button"
            aria-label="Create the page pact"
            disabled={create.isPending}
            onClick={() => {
              create.mutate();
            }}
            className={SMALL_ACTION}
          >
            Create
          </button>
        )}
      </span>
      {create.error && (
        <span
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(create.error)}
        </span>
      )}
    </span>
  );
}

/**
 * Restore a retired page (on its own, or back under the page it was part
 * of while that page is still there), and delete it with the app's
 * two-step confirm. Drawn in the retired banner only.
 */
function PageLifecycle({
  domain,
  path,
  page,
  onDelete,
  deleting,
  focusRestore = false,
  onRestoreFocused,
  onRestored,
}: {
  domain: string;
  path: string;
  page: PageView;
  onDelete: () => void;
  deleting: boolean;
  /** Take the focus onto Restore (after a retire from the menu). */
  focusRestore?: boolean;
  onRestoreFocused?: () => void;
  onRestored?: () => void;
}): ReactElement {
  const queryClient = useQueryClient();
  const restoreRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    if (!focusRestore) {
      return;
    }
    restoreRef.current?.focus();
    onRestoreFocused?.();
  }, [focusRestore, onRestoreFocused]);
  const restore = useMutation({
    mutationFn: (owner: string | undefined) =>
      restoreReflection(domain, path, owner),
    onSuccess: () => {
      onRestored?.();
      invalidateReflection(queryClient, domain);
    },
  });
  const owner = page.retired?.wasOwnedBy ?? null;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <button
          ref={restoreRef}
          type="button"
          disabled={restore.isPending}
          onClick={() => {
            restore.mutate(undefined);
          }}
          className={BUTTON.secondary}
        >
          Restore
        </button>
        {owner?.exists === true && (
          <button
            type="button"
            disabled={restore.isPending}
            onClick={() => {
              restore.mutate(owner.path);
            }}
            className={BUTTON.secondary}
          >
            {`Restore under ${owner.title}`}
          </button>
        )}
        <DestructiveAction
          label="Delete"
          confirmLabel="Confirm delete"
          pending={deleting}
          onConfirm={onDelete}
        />
      </div>
      {restore.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-2 py-1 text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(restore.error)}
        </p>
      )}
    </div>
  );
}

/**
 * Over a retired page: "Retired: no longer part of Architecture overview",
 * or just "Retired" for a page that was on its own, with restore and delete
 * for a curator. After a retire from the page's menu, Restore takes the
 * focus.
 */
export function RetiredBanner({
  domain,
  path,
  page,
  curator,
  onDelete,
  deleting,
  focusRestore = false,
  onRestoreFocused,
  onRestored,
}: {
  domain: string;
  path: string;
  page: PageView;
  curator: boolean;
  onDelete: () => void;
  deleting: boolean;
  /** Take the focus onto Restore once the banner shows. */
  focusRestore?: boolean;
  /** Told when Restore took the focus. */
  onRestoreFocused?: () => void;
  /** Told when a restore landed, so the page can place the focus. */
  onRestored?: () => void;
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
          focusRestore={focusRestore}
          {...(onRestoreFocused === undefined ? {} : { onRestoreFocused })}
          {...(onRestored === undefined ? {} : { onRestored })}
        />
      )}
    </aside>
  );
}
