/**
 * Account administration, the admin-only screen, built for hundreds of
 * accounts: a table to find them, a detail panel to change one, and a bulk
 * bar to change many.
 *
 * The table holds no edit fields. It is searched, filtered, sorted and paged
 * by the server, and all of that lives in the address (see `usersQuery.ts`),
 * so a list can be linked and the back button walks through it. A row opens
 * the account at `/admin/users/<name>`: beside the table at wide widths, the
 * whole page at narrow ones.
 *
 * Every refusal is the server's own sentence (NOT_LAST_ADMIN above all), shown
 * where the action happened; the force escape hatches exist only in the CLI,
 * on purpose. Nothing here asks the server whether an action is allowed
 * before offering it. The one thing the screen declines to draw is a way for
 * the caller to deactivate their own account: no deactivate in their panel,
 * and no checkbox on their row.
 */

import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { ArrowDown, ArrowUp } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";

import { problemDetail } from "../api/client";
import type {
  BulkAction,
  BulkResult,
  KindFilter,
  ReflectionLevel,
  Role,
  SortColumn,
  StateFilter,
  User,
} from "../api/model";
import type { DiscardedDrafts, UsersPageRequest } from "../api/users";
import {
  USERS_QUERY_KEY,
  bulkUsers,
  createUser,
  discardUserDrafts,
  fetchUsersPage,
  usersPageKey,
} from "../api/users";
import { useAuth } from "../auth/AuthContext";
import { focusLeft } from "../focusLeft";
import { Loading } from "../components/Loading";
import {
  BUTTON,
  CONTROL_HEIGHT,
  FIELD,
  FOCUS_RING,
} from "../components/primitives";
import { ReflectionLevelSelect } from "../components/ReflectionLevelSelect";
import { reflectionAfter } from "../components/reflectionLevels";
import { formatDay } from "../format";
import { usersRoute } from "../paths";
import { refreshAfterWrite } from "../query/refreshAfterWrite";
import { usePageTitle } from "../usePageTitle";
import NotFound from "./NotFound";
import type { Notice } from "./accounts";
import { ROLES, signInWords } from "./accounts";
import { UserDetail } from "./UserDetail";
import {
  PAGE_SIZES,
  isFiltered,
  narrowed,
  readUsersSearch,
  writeUsersSearch,
} from "./usersQuery";

const SECONDARY = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.secondary}`;
const DANGER_BUTTON = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.destructive}`;
const LABEL = "text-xs text-slate-500 dark:text-slate-400";

/** How long typing in the search waits before the list follows it. */
export const SEARCH_DELAY_MS = 300;

/** The table's columns, in order, with the sort each one asks for. */
const COLUMNS: readonly { sort: SortColumn; label: string }[] = [
  { sort: "name", label: "Name" },
  { sort: "display", label: "Display name" },
  { sort: "role", label: "Role" },
  { sort: "state", label: "State" },
  { sort: "kind", label: "Sign-in" },
  { sort: "last_seen", label: "Last seen" },
];

const KIND_OPTIONS: readonly { value: KindFilter; label: string }[] = [
  { value: "password", label: "Password" },
  { value: "oidc", label: "SSO" },
  { value: "proxy", label: "Proxy" },
  { value: "token_only", label: "Token only" },
];

export default function UsersAdmin() {
  const { capabilities } = useAuth();
  usePageTitle(capabilities.canAdminister ? "Users" : null);
  if (!capabilities.canAdminister) {
    return <NotFound />;
  }
  return <UsersScreen />;
}

/** The address of the list or of one account, keeping the list's search. */
function usersAddress(search: URLSearchParams, name?: string): string {
  const query = search.toString();
  const path =
    name === undefined
      ? usersRoute()
      : `${usersRoute()}/${encodeURIComponent(name)}`;
  return query === "" ? path : `${path}?${query}`;
}

function UsersScreen() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const { user } = useAuth();
  const { name: openName } = useParams();
  const [searchParams, setSearchParams] = useSearchParams();
  const request = readUsersSearch(searchParams);
  const requestKey = JSON.stringify(request);
  const [notice, setNotice] = useState<Notice | null>(null);

  /** Change what is listed; typing replaces the history entry, the rest push. */
  function setRequest(
    change: (current: UsersPageRequest) => UsersPageRequest,
    replace = false,
  ) {
    setSearchParams(
      (current) => writeUsersSearch(change(readUsersSearch(current))),
      { replace },
    );
  }

  const listing = useQuery({
    queryKey: usersPageKey(request),
    queryFn: () => fetchUsersPage(request),
    placeholderData: keepPreviousData,
  });
  const accounts = listing.data?.users ?? [];
  const total = listing.data?.total ?? 0;
  const lastPage = Math.max(1, Math.ceil(total / request.size));

  // A page past the end (a bookmark, or the last account of the last page
  // deactivated away) goes to the last page there is, without a word.
  const pastTheEnd =
    listing.data !== undefined &&
    !listing.isPlaceholderData &&
    request.page > lastPage;
  useEffect(() => {
    if (pastTheEnd) {
      setRequest((current) => ({ ...current, page: lastPage }), true);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pastTheEnd, lastPage]);

  // The selection belongs to one page of one list: a different page, filter
  // or sort starts with nothing selected.
  const [selection, setSelection] = useState<{
    key: string;
    names: ReadonlySet<string>;
  }>({ key: requestKey, names: new Set() });
  const selected =
    selection.key === requestKey ? selection.names : new Set<string>();
  const selectable = accounts.filter((account) => account.name !== user?.name);

  function select(names: ReadonlySet<string>) {
    setSelection({ key: requestKey, names });
  }

  const [results, setResults] = useState<{
    said: string;
    rows: BulkResult[];
  } | null>(null);

  const bulk = useMutation({
    mutationFn: bulkUsers,
    onMutate: () => {
      setNotice(null);
      setResults(null);
    },
    onSuccess: (rows, body) => {
      setResults({ said: bulkWords(body.action, body.role ?? null), rows });
      select(new Set());
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
    onSettled: () =>
      void refreshAfterWrite(queryClient, { queryKey: USERS_QUERY_KEY }),
  });

  const add = useMutation({
    mutationFn: createUser,
    onSuccess: (created) => {
      setNotice({ kind: "done", text: `Added ${created.name}.` });
      void refreshAfterWrite(queryClient, { queryKey: USERS_QUERY_KEY });
      void navigate(usersAddress(searchParams, created.name));
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const endDrafts = useMutation({
    mutationFn: discardUserDrafts,
    onSuccess: ({ domains, error }, name) => {
      setNotice({
        kind:
          error ||
          domains.some((row) => row.error ?? row.not_hosted) ||
          domains.some((row) => row.left.length > 0)
            ? "problem"
            : "done",
        text: endedDraftsText(name, domains, error),
      });
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const listTo = usersAddress(searchParams);
  const open = openName !== undefined;

  return (
    <div className="flex flex-col gap-6">
      <header>
        <h1 className="text-display">Users</h1>
        <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
          Who may sign in to this instance, and what each of them may do. The
          server decides every change; what it refuses is said here in its own
          words.
        </p>
      </header>

      {notice && (
        <p
          role={notice.kind === "problem" ? "alert" : "status"}
          className={
            notice.kind === "problem"
              ? "rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
              : "rounded border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300"
          }
        >
          {notice.text}
        </p>
      )}

      <div
        className={
          open
            ? "2xl:grid 2xl:grid-cols-[minmax(0,1fr)_20rem] 2xl:items-start 2xl:gap-6"
            : undefined
        }
      >
        <div
          className={`flex min-w-0 flex-col gap-4 ${open ? "hidden 2xl:flex" : ""}`}
        >
          <SearchAndFilters request={request} onChange={setRequest} />

          {results && (
            <BulkResults
              said={results.said}
              rows={results.rows}
              onDismiss={() => {
                setResults(null);
              }}
            />
          )}

          {selected.size > 0 && (
            <BulkBar
              count={selected.size}
              pending={bulk.isPending}
              onRun={(action, role) => {
                bulk.mutate({
                  names: [...selected],
                  action,
                  ...(action === "role" ? { role } : {}),
                });
              }}
              onClear={() => {
                select(new Set());
              }}
            />
          )}

          {request.sort === "last_seen" && (
            <p className="text-sm text-slate-500 dark:text-slate-400">
              Sorted by last seen: an account that signs in while you page can
              move to another page.
            </p>
          )}

          {listing.isPending && <AccountsSkeleton />}

          {listing.error && (
            <p
              role="alert"
              className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
            >
              {problemDetail(listing.error)}
            </p>
          )}

          {listing.data && accounts.length === 0 && (
            <p className="rounded border border-dashed border-slate-300 px-3 py-6 text-sm text-slate-500 dark:border-slate-700 dark:text-slate-400">
              {isFiltered(request)
                ? "No account matches."
                : "No accounts yet. Add one below, or let a trusted-header proxy provision them as people arrive through it."}
            </p>
          )}

          {accounts.length > 0 && (
            <AccountsTable
              accounts={accounts}
              request={request}
              self={user?.name}
              openName={openName}
              selected={selected}
              allSelected={
                selectable.length > 0 &&
                selectable.every((account) => selected.has(account.name))
              }
              linkTo={(name) => usersAddress(searchParams, name)}
              onSort={(sort) => {
                setRequest((current) =>
                  narrowed(current, {
                    sort,
                    dir:
                      current.sort === sort
                        ? current.dir === "asc"
                          ? "desc"
                          : "asc"
                        : sort === "last_seen"
                          ? "desc"
                          : "asc",
                  }),
                );
              }}
              onSelect={(name, on) => {
                const next = new Set(selected);
                if (on) next.add(name);
                else next.delete(name);
                select(next);
              }}
              onSelectAll={(on) => {
                select(
                  on
                    ? new Set(selectable.map((account) => account.name))
                    : new Set(),
                );
              }}
            />
          )}

          {listing.data && total > 0 && (
            <Pager
              request={request}
              total={total}
              shown={accounts.length}
              lastPage={lastPage}
              onPage={(page) => {
                setRequest((current) => ({ ...current, page }));
              }}
              onSize={(size) => {
                setRequest((current) => narrowed(current, { size }));
              }}
            />
          )}

          <CreateForm
            pending={add.isPending}
            onCreate={(body) => add.mutateAsync(body)}
          />

          <EndDraftsForm
            pending={endDrafts.isPending}
            onEnd={(name) => endDrafts.mutateAsync(name)}
          />
        </div>

        {open && (
          <UserDetail
            name={openName}
            fromList={accounts.find((account) => account.name === openName)}
            isSelf={openName === user?.name}
            listTo={listTo}
            onNotice={setNotice}
          />
        )}
      </div>
    </div>
  );
}

/**
 * The search field and the filters.
 *
 * Typing follows into the address after a short pause, replacing the history
 * entry so the back button does not step through every letter; Enter applies
 * at once. A filter applies when it is chosen.
 */
function SearchAndFilters({
  request,
  onChange,
}: {
  request: UsersPageRequest;
  onChange: (
    change: (current: UsersPageRequest) => UsersPageRequest,
    replace?: boolean,
  ) => void;
}) {
  const ids = {
    q: useId(),
    role: useId(),
    state: useId(),
    kind: useId(),
    github: useId(),
    after: useId(),
    before: useId(),
  };
  const [text, setText] = useState(request.q);
  // The search the address last got from this field. When the address moves
  // on its own (the back button, a link), the field follows it; while the
  // person types, it does not, or a late answer would undo their typing.
  const [applied, setApplied] = useState(request.q);
  if (request.q !== applied) {
    setApplied(request.q);
    setText(request.q);
  }

  function apply(value: string, replace: boolean) {
    setApplied(value);
    onChange((current) => narrowed(current, { q: value }), replace);
  }

  useEffect(() => {
    if (text === applied) return;
    const timer = window.setTimeout(() => {
      apply(text, true);
    }, SEARCH_DELAY_MS);
    return () => {
      window.clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, applied]);

  const githubValue =
    request.github === null ? "" : request.github ? "true" : "false";

  return (
    <div
      role="search"
      aria-label="Find accounts"
      className="flex flex-col gap-3"
    >
      <form
        className="flex flex-col gap-1"
        onSubmit={(event) => {
          event.preventDefault();
          apply(text, false);
        }}
      >
        <label htmlFor={ids.q} className={LABEL}>
          Search
        </label>
        <input
          id={ids.q}
          type="search"
          value={text}
          autoComplete="off"
          placeholder="Name, display name or email"
          onChange={(event) => {
            setText(event.target.value);
          }}
          className={`w-full max-w-md ${FIELD}`}
        />
      </form>
      <div className="flex flex-wrap items-end gap-3">
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.role} className={LABEL}>
            Role
          </label>
          <select
            id={ids.role}
            value={request.role ?? ""}
            onChange={(event) => {
              const role = (event.target.value || null) as Role | null;
              onChange((current) => narrowed(current, { role }));
            }}
            className={FIELD}
          >
            <option value="">Any role</option>
            {ROLES.map((role) => (
              <option key={role} value={role}>
                {role}
              </option>
            ))}
          </select>
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.state} className={LABEL}>
            State
          </label>
          <select
            id={ids.state}
            value={request.state ?? ""}
            onChange={(event) => {
              const state = (event.target.value || null) as StateFilter | null;
              onChange((current) => narrowed(current, { state }));
            }}
            className={FIELD}
          >
            <option value="">Any state</option>
            <option value="active">Active</option>
            <option value="disabled">Disabled</option>
          </select>
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.kind} className={LABEL}>
            Sign-in
          </label>
          <select
            id={ids.kind}
            value={request.kind ?? ""}
            onChange={(event) => {
              const kind = (event.target.value || null) as KindFilter | null;
              onChange((current) => narrowed(current, { kind }));
            }}
            className={FIELD}
          >
            <option value="">Any sign-in</option>
            {KIND_OPTIONS.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.github} className={LABEL}>
            GitHub
          </label>
          <select
            id={ids.github}
            value={githubValue}
            onChange={(event) => {
              const value = event.target.value;
              const github = value === "" ? null : value === "true";
              onChange((current) => narrowed(current, { github }));
            }}
            className={FIELD}
          >
            <option value="">Linked or not</option>
            <option value="true">Linked</option>
            <option value="false">Not linked</option>
          </select>
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.after} className={LABEL}>
            Seen after
          </label>
          <input
            id={ids.after}
            type="date"
            value={request.seenAfter ?? ""}
            onChange={(event) => {
              const seenAfter = event.target.value || null;
              onChange((current) => narrowed(current, { seenAfter }));
            }}
            className={FIELD}
          />
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={ids.before} className={LABEL}>
            Seen before
          </label>
          <input
            id={ids.before}
            type="date"
            value={request.seenBefore ?? ""}
            onChange={(event) => {
              const seenBefore = event.target.value || null;
              onChange((current) => narrowed(current, { seenBefore }));
            }}
            className={FIELD}
          />
        </div>
        {isFiltered(request) && (
          <button
            type="button"
            onClick={() => {
              setApplied("");
              setText("");
              onChange((current) =>
                narrowed(current, {
                  q: "",
                  role: null,
                  state: null,
                  kind: null,
                  github: null,
                  seenAfter: null,
                  seenBefore: null,
                }),
              );
            }}
            className={SECONDARY}
          >
            Clear filters
          </button>
        )}
      </div>
    </div>
  );
}

/**
 * The accounts of one page. Each column header sorts by it; the login name
 * opens the account. No cell is an edit field.
 */
function AccountsTable({
  accounts,
  request,
  self,
  openName,
  selected,
  allSelected,
  linkTo,
  onSort,
  onSelect,
  onSelectAll,
}: {
  accounts: User[];
  request: UsersPageRequest;
  self: string | undefined;
  openName: string | undefined;
  selected: ReadonlySet<string>;
  allSelected: boolean;
  linkTo: (name: string) => string;
  onSort: (sort: SortColumn) => void;
  onSelect: (name: string, on: boolean) => void;
  onSelectAll: (on: boolean) => void;
}) {
  const selectAll = useRef<HTMLInputElement>(null);
  const some = selected.size > 0 && !allSelected;
  useEffect(() => {
    if (selectAll.current) selectAll.current.indeterminate = some;
  }, [some]);

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <caption className="sr-only">Accounts on this page</caption>
        <thead className="text-caption font-semibold text-slate-500 dark:text-slate-400">
          <tr>
            <th scope="col" className="w-8 px-2 py-2">
              <input
                ref={selectAll}
                type="checkbox"
                aria-label="Select every account on this page"
                checked={allSelected}
                onChange={(event) => {
                  onSelectAll(event.target.checked);
                }}
              />
            </th>
            {COLUMNS.map((column) => {
              const sorted = request.sort === column.sort;
              return (
                <th
                  key={column.sort}
                  scope="col"
                  aria-sort={
                    sorted
                      ? request.dir === "asc"
                        ? "ascending"
                        : "descending"
                      : undefined
                  }
                  className="px-2 py-2 whitespace-nowrap"
                >
                  <button
                    type="button"
                    onClick={() => {
                      onSort(column.sort);
                    }}
                    className={`inline-flex items-center gap-1 rounded hover:text-slate-900 dark:hover:text-slate-100 ${FOCUS_RING} ${sorted ? "text-slate-900 dark:text-slate-100" : ""}`}
                  >
                    {column.label}
                    {sorted &&
                      (request.dir === "asc" ? (
                        <ArrowUp aria-hidden="true" className="h-3 w-3" />
                      ) : (
                        <ArrowDown aria-hidden="true" className="h-3 w-3" />
                      ))}
                  </button>
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
          {accounts.map((account) => {
            const lastSeen = account.last_seen ?? null;
            const here = account.name === openName;
            return (
              <tr
                key={account.name}
                className={here ? "bg-accent-tint" : undefined}
              >
                <td className="px-2 py-2">
                  {account.name !== self && (
                    <input
                      type="checkbox"
                      aria-label={`Select ${account.name}`}
                      checked={selected.has(account.name)}
                      onChange={(event) => {
                        onSelect(account.name, event.target.checked);
                      }}
                    />
                  )}
                </td>
                <th scope="row" className="px-2 py-2 font-normal">
                  <Link
                    to={linkTo(account.name)}
                    aria-current={here ? "page" : undefined}
                    className={`rounded font-mono break-all text-link underline-offset-2 hover:underline ${FOCUS_RING}`}
                  >
                    {account.name}
                  </Link>
                </th>
                <td className="px-2 py-2 break-words">{account.display}</td>
                <td className="px-2 py-2">{account.role}</td>
                <td className="px-2 py-2">
                  {account.disabled ? "Disabled" : "Active"}
                </td>
                <td className="px-2 py-2 whitespace-nowrap">
                  {signInWords(account.sign_in)}
                </td>
                <td className="px-2 py-2 whitespace-nowrap tabular-nums">
                  {lastSeen === null ? (
                    <span className="text-slate-500 dark:text-slate-400">
                      Never
                    </span>
                  ) : (
                    formatDay(lastSeen)
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** Where the page stands in the list, the way to the next one, and the page size. */
function Pager({
  request,
  total,
  shown,
  lastPage,
  onPage,
  onSize,
}: {
  request: UsersPageRequest;
  total: number;
  shown: number;
  lastPage: number;
  onPage: (page: number) => void;
  onSize: (size: number) => void;
}) {
  const sizeId = useId();
  const first = (request.page - 1) * request.size + 1;
  const last = first + shown - 1;
  return (
    <nav
      aria-label="Pages"
      className="flex flex-wrap items-center gap-3 text-sm"
    >
      <p className="tabular-nums">
        {shown === 0
          ? `${String(total)} accounts`
          : `${String(first)} to ${String(last)} of ${String(total)}`}
      </p>
      <div className="flex items-center gap-2">
        <button
          type="button"
          disabled={request.page <= 1}
          onClick={() => {
            onPage(request.page - 1);
          }}
          className={SECONDARY}
        >
          Previous page
        </button>
        <span className="tabular-nums">
          Page {request.page} of {lastPage}
        </span>
        <button
          type="button"
          disabled={request.page >= lastPage}
          onClick={() => {
            onPage(request.page + 1);
          }}
          className={SECONDARY}
        >
          Next page
        </button>
      </div>
      <div className="flex items-center gap-2">
        <label htmlFor={sizeId} className={LABEL}>
          Per page
        </label>
        <select
          id={sizeId}
          value={request.size}
          onChange={(event) => {
            onSize(Number(event.target.value));
          }}
          className={FIELD}
        >
          {PAGE_SIZES.map((size) => (
            <option key={size} value={size}>
              {size}
            </option>
          ))}
        </select>
      </div>
    </nav>
  );
}

/** What a bulk action is, as the confirm and the results say it. */
function bulkWords(action: BulkAction, role: Role | null): string {
  switch (action) {
    case "deactivate":
      return "Deactivate";
    case "reactivate":
      return "Reactivate";
    case "role":
      return `Set role ${role ?? ""}`.trim();
  }
}

/** "1 account" or "3 accounts". */
function accountsWord(count: number): string {
  return count === 1 ? "1 account" : `${String(count)} accounts`;
}

/**
 * The selection's actions. Each one asks once more, naming the action and how
 * many accounts it touches, before it goes to the server.
 */
function BulkBar({
  count,
  pending,
  onRun,
  onClear,
}: {
  count: number;
  pending: boolean;
  onRun: (action: BulkAction, role: Role) => void;
  onClear: () => void;
}) {
  const roleId = useId();
  const [role, setRole] = useState<Role>("viewer");
  const [asking, setAsking] = useState<BulkAction | null>(null);
  const firstStep = useRef<HTMLDivElement>(null);
  const question =
    asking === null
      ? ""
      : `${bulkWords(asking, role)} for ${accountsWord(count)}?`;

  function giveUp() {
    setAsking(null);
    firstStep.current?.querySelector("button")?.focus();
  }

  return (
    <section
      aria-label="Selected accounts"
      className="flex flex-col gap-3 rounded border border-slate-200 bg-slate-50 p-3 text-sm dark:border-slate-800 dark:bg-slate-900"
      onKeyDown={(event) => {
        if (event.key === "Escape" && asking !== null) {
          event.stopPropagation();
          giveUp();
        }
      }}
    >
      <div ref={firstStep} className="flex flex-wrap items-center gap-2">
        <span className="font-medium">{accountsWord(count)} selected</span>
        <button
          type="button"
          disabled={pending}
          onClick={() => {
            setAsking("deactivate");
          }}
          className={SECONDARY}
        >
          Deactivate
        </button>
        <button
          type="button"
          disabled={pending}
          onClick={() => {
            setAsking("reactivate");
          }}
          className={SECONDARY}
        >
          Reactivate
        </button>
        <label htmlFor={roleId} className="sr-only">
          New role for the selected accounts
        </label>
        <select
          id={roleId}
          value={role}
          onChange={(event) => {
            setRole(event.target.value as Role);
            setAsking(null);
          }}
          className={FIELD}
        >
          {ROLES.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
        <button
          type="button"
          disabled={pending}
          onClick={() => {
            setAsking("role");
          }}
          className={SECONDARY}
        >
          Set role
        </button>
        <button
          type="button"
          onClick={onClear}
          className={`${CONTROL_HEIGHT} ${BUTTON.ghost}`}
        >
          Clear selection
        </button>
      </div>
      {asking !== null && (
        <div
          className="flex flex-wrap items-center gap-2"
          onBlur={(event) => {
            if (focusLeft(event.currentTarget, event.relatedTarget)) {
              setAsking(null);
            }
          }}
        >
          <p>{question}</p>
          <button
            type="button"
            autoFocus
            disabled={pending}
            onClick={() => {
              onRun(asking, role);
              setAsking(null);
            }}
            className={asking === "deactivate" ? DANGER_BUTTON : SECONDARY}
          >
            {`${bulkWords(asking, role)} ${accountsWord(count)}`}
          </button>
          <button type="button" onClick={giveUp} className={SECONDARY}>
            Cancel
          </button>
        </div>
      )}
    </section>
  );
}

/** What a bulk action did, account by account, refusals first. */
function BulkResults({
  said,
  rows,
  onDismiss,
}: {
  said: string;
  rows: BulkResult[];
  onDismiss: () => void;
}) {
  const refused = rows.filter((row) => !row.done);
  const done = rows.filter((row) => row.done);
  return (
    <section
      aria-label="Bulk results"
      role={refused.length > 0 ? "alert" : "status"}
      className={`flex flex-col gap-2 rounded border px-3 py-2 text-sm ${
        refused.length > 0
          ? "border-red-300 bg-red-50 text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
          : "border-slate-200 bg-slate-50 text-slate-700 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300"
      }`}
    >
      <div className="flex items-center justify-between gap-3">
        <p className="font-medium">
          {said}: {String(done.length)} done
          {refused.length > 0 ? `, ${String(refused.length)} refused` : ""}.
        </p>
        <button
          type="button"
          onClick={onDismiss}
          className={`${CONTROL_HEIGHT} ${BUTTON.ghost}`}
        >
          Dismiss
        </button>
      </div>
      <ul className="flex flex-col gap-0.5">
        {[...refused, ...done].map((row) => (
          <li key={row.name}>
            <span className="font-mono">{row.name}</span>
            {": "}
            {row.done ? "done" : (row.message ?? "refused")}
          </li>
        ))}
      </ul>
    </section>
  );
}

/** What ending a name's drafts did, in one sentence per domain. */
function endedDraftsText(
  name: string,
  domains: DiscardedDrafts[],
  error?: string,
): string {
  if (domains.length === 0) {
    return error ? `${error}.` : `${name} holds no drafts.`;
  }
  const lines = domains.map((row) => {
    const parts = [`${row.domain}: ${row.discarded} ended`];
    if (row.left.length > 0) {
      parts.push(`${row.left.length} left, open in an editor`);
    }
    if (row.where) {
      parts.push(row.where);
    }
    if (row.error) {
      parts.push(`stopped: ${row.error}`);
    }
    return parts.join(", ");
  });
  const also = error ? ` ${error}.` : "";
  return `Drafts under ${name}. ${lines.join(". ")}.${also}`;
}

/**
 * The way out when a new account is refused because its name still holds a
 * removed account's drafts. Nothing ends a draft without an admin asking, so the
 * name is typed twice: what goes cannot be brought back.
 */
function EndDraftsForm({
  pending,
  onEnd,
}: {
  pending: boolean;
  onEnd: (name: string) => Promise<unknown>;
}) {
  const nameField = useId();
  const confirmField = useId();
  const [name, setName] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const ready = name.trim() !== "" && confirmation === name;

  return (
    <section
      aria-labelledby="users-end-drafts"
      className="rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <h2 id="users-end-drafts" className="text-lg font-semibold">
        End a removed account&apos;s drafts
      </h2>
      <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
        A new account is refused while its name still holds the drafts of a
        removed account. This ends them, and the texts kept from them, for good.
        It does not work for a name with an account.
      </p>
      <form
        className="mt-4 flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (!ready) {
            return;
          }
          void onEnd(name).then(
            () => {
              setName("");
              setConfirmation("");
            },
            () => undefined,
          );
        }}
      >
        <div className="flex flex-col gap-1">
          <label htmlFor={nameField} className="text-sm font-medium">
            Removed account&apos;s name
          </label>
          <input
            id={nameField}
            className={FIELD}
            value={name}
            autoComplete="off"
            onChange={(event) => {
              setName(event.target.value);
            }}
          />
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={confirmField} className="text-sm font-medium">
            Type the name again
          </label>
          <input
            id={confirmField}
            className={FIELD}
            value={confirmation}
            autoComplete="off"
            onChange={(event) => {
              setConfirmation(event.target.value);
            }}
          />
        </div>
        <button
          type="submit"
          disabled={pending || !ready}
          className={DANGER_BUTTON}
        >
          End these drafts
        </button>
      </form>
    </section>
  );
}

/**
 * Add an account. The password is the initial one; the account can be given a
 * new one from its details.
 *
 * The fields are emptied only once the server has taken the account. A name
 * already in use is the commonest refusal here, and a form that wiped itself
 * on the way to hearing that would make the fix retyping everything.
 */
function CreateForm({
  pending,
  onCreate,
}: {
  pending: boolean;
  onCreate: (body: Parameters<typeof createUser>[0]) => Promise<unknown>;
}) {
  const nameField = useId();
  const displayField = useId();
  const roleField = useId();
  const reflectionField = useId();
  const passwordField = useId();
  const [name, setName] = useState("");
  const [display, setDisplay] = useState("");
  const [role, setRole] = useState<Role>("viewer");
  const [reflection, setReflection] = useState<ReflectionLevel>("default");
  const [password, setPassword] = useState("");

  return (
    <section
      aria-labelledby="users-create"
      className="rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <h2 id="users-create" className="text-lg font-semibold">
        Add an account
      </h2>
      <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
        The login name folds to lower case; the display name is what the rest of
        the app calls them, and defaults to the login name.
      </p>
      <form
        className="mt-4 flex flex-wrap items-end gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          void onCreate({
            name,
            password,
            role,
            ...(reflection === "default" ? {} : { reflection }),
            ...(display === "" ? {} : { display }),
          }).then(
            () => {
              setName("");
              setDisplay("");
              setRole("viewer");
              setReflection("default");
              setPassword("");
            },
            // The refusal is already on screen, put there by the mutation
            // that owns it. Here it only means: keep what was typed.
            () => undefined,
          );
        }}
      >
        <div className="flex flex-col gap-1">
          <label
            htmlFor={nameField}
            className="text-xs text-slate-500 dark:text-slate-400"
          >
            Login name
          </label>
          <input
            id={nameField}
            required
            autoComplete="off"
            value={name}
            onChange={(event) => {
              setName(event.target.value);
            }}
            className={FIELD}
          />
        </div>

        <div className="flex flex-col gap-1">
          <label
            htmlFor={displayField}
            className="text-xs text-slate-500 dark:text-slate-400"
          >
            Display name
          </label>
          <input
            id={displayField}
            autoComplete="off"
            value={display}
            onChange={(event) => {
              setDisplay(event.target.value);
            }}
            className={FIELD}
          />
        </div>

        <div className="flex flex-col gap-1">
          <label
            htmlFor={roleField}
            className="text-xs text-slate-500 dark:text-slate-400"
          >
            Role
          </label>
          <select
            id={roleField}
            value={role}
            onChange={(event) => {
              const next = event.target.value as Role;
              setRole(next);
              setReflection((current) => reflectionAfter(next, current));
            }}
            className={FIELD}
          >
            {ROLES.map((option) => (
              <option key={option} value={option}>
                {option}
              </option>
            ))}
          </select>
        </div>

        <div className="flex flex-col gap-1">
          <label
            htmlFor={reflectionField}
            className="text-xs text-slate-500 dark:text-slate-400"
          >
            Reflection
          </label>
          <ReflectionLevelSelect
            id={reflectionField}
            label="Reflection"
            knowledge={role}
            value={reflection}
            onChange={setReflection}
          />
        </div>

        <div className="flex flex-col gap-1">
          <label
            htmlFor={passwordField}
            className="text-xs text-slate-500 dark:text-slate-400"
          >
            Password
          </label>
          <input
            id={passwordField}
            type="password"
            required
            autoComplete="new-password"
            value={password}
            onChange={(event) => {
              setPassword(event.target.value);
            }}
            className={FIELD}
          />
        </div>

        {/*
          The one primary on the screen, in the app's own accent rather than
          the stray blue this was written in before the tiers existed.
        */}
        <button
          type="submit"
          disabled={pending}
          className={`${CONTROL_HEIGHT} ${BUTTON.primary}`}
        >
          Add user
        </button>
      </form>
    </section>
  );
}

/** The accounts listing's loading block, in the app's one loading look. */
function AccountsSkeleton() {
  return <Loading label="Loading accounts" subject="the accounts" />;
}
