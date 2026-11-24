/**
 * One account and every lever an admin has on it: display name, role,
 * Reflection level, state, write list, password reset, GitHub link and
 * delete, and what it holds in drafts.
 *
 * It has its own address, `/admin/users/<name>`, so it can be linked and the
 * back button closes it. Beside the table at wide widths, the whole page at
 * narrow ones; the list screen decides which.
 *
 * The panel reads the account with `GET /users/{name}`. The row the table
 * already holds is shown while that read runs, so opening a row draws at
 * once; the write list and drafts follow when the read answers.
 *
 * Opened from the list, the panel takes the focus on its heading; the list
 * hands it back to the row when the panel closes.
 *
 * Nothing here asks the server whether an action is allowed before offering
 * it; the server decides and its sentence comes back through `onNotice`. The
 * one control left out is the caller's own deactivate, which is not a guess
 * about the rules but a refusal to offer somebody the key to lock themselves
 * out with.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useRef, useState } from "react";
import { Link, useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import type { DraftCount, PatchUserBody, Role, User } from "../api/model";
import type { AccountDetail, UsersPage } from "../api/users";
import {
  USERS_QUERY_KEY,
  deleteUser,
  fetchUser,
  patchUser,
  resetPassword,
  setUserWrites,
  unlinkUserGithub,
  userKey,
} from "../api/users";
import { focusLeft } from "../focusLeft";
import {
  BUTTON,
  CONTROL_HEIGHT,
  FIELD,
  FOCUS_RING,
} from "../components/primitives";
import { ReflectionLevelSelect } from "../components/ReflectionLevelSelect";
import { raiseForNone } from "../components/reflectionLevels";
import { formatDay } from "../format";
import { refreshAfterWrite } from "../query/refreshAfterWrite";
import type { Notice } from "./accounts";
import { ROLES, signInWords } from "./accounts";
import { personLabelWithLogin } from "../people";

const SECONDARY = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.secondary}`;
const DANGER = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.destructive}`;
const TERM = "text-xs text-slate-500 dark:text-slate-400";

export function UserDetail({
  name,
  fromList,
  isSelf,
  listTo,
  focusOnOpen,
  onNotice,
}: {
  name: string;
  /** The row the table holds for this name, shown while the panel reads it. */
  fromList: User | undefined;
  isSelf: boolean;
  /** The list's address, with its search, filters and page. */
  listTo: string;
  /**
   * Whether to put the focus on the heading once it is drawn: true when the
   * panel was opened from the list, false for an address opened directly.
   * Read once; the list mounts one panel per account.
   */
  focusOnOpen: boolean;
  onNotice: (notice: Notice | null) => void;
}) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const account = useQuery({
    queryKey: userKey(name),
    queryFn: () => fetchUser(name),
    placeholderData: (): AccountDetail | undefined =>
      fromList && { user: fromList, writes: null, drafts: null, loaded: false },
  });

  // The heading takes the focus the first time it is drawn, whichever of
  // the two headings that is.
  const wantsFocus = useRef(focusOnOpen);
  const heading = useCallback((node: HTMLHeadingElement | null) => {
    if (node && wantsFocus.current) {
      wantsFocus.current = false;
      node.focus();
    }
  }, []);

  const invalidate = () =>
    refreshAfterWrite(queryClient, { queryKey: USERS_QUERY_KEY });

  // The account as a notice names it: "Display (login)", so the sentence says
  // who in words and still names the account exactly.
  const who = personLabelWithLogin(
    name,
    account.data?.user.display ?? fromList?.display,
  );

  const patch = useMutation({
    mutationFn: (body: PatchUserBody) => patchUser(name, body),
    // Optimistic for role, display, Reflection and state: apply to the panel
    // and to every cached page, roll back on refusal, resettle from the server.
    onMutate: async (body) => {
      await queryClient.cancelQueries({ queryKey: USERS_QUERY_KEY });
      const one = queryClient.getQueryData<AccountDetail | null>(userKey(name));
      const pages = queryClient.getQueriesData<UsersPage>({
        queryKey: [...USERS_QUERY_KEY, "page"],
      });
      const current = one ?? account.data;
      if (current) {
        queryClient.setQueryData<AccountDetail>(userKey(name), {
          ...current,
          user: withPatch(current.user, body),
        });
      }
      queryClient.setQueriesData<UsersPage>(
        { queryKey: [...USERS_QUERY_KEY, "page"] },
        (page) =>
          page && {
            ...page,
            users: page.users.map((row) =>
              row.name === name ? withPatch(row, body) : row,
            ),
          },
      );
      onNotice(null);
      return { one, pages };
    },
    onError: (error: Error, _body, context) => {
      if (context) {
        // A panel still on the table's row had no copy of its own: drop the
        // optimistic one, and the rolled back row shows through again.
        if (context.one === undefined) {
          queryClient.removeQueries({ queryKey: userKey(name), exact: true });
        } else {
          queryClient.setQueryData(userKey(name), context.one);
        }
        for (const [key, page] of context.pages) {
          queryClient.setQueryData(key, page);
        }
      }
      onNotice({ kind: "problem", text: problemDetail(error) });
    },
    onSettled: () => void invalidate(),
  });

  const reset = useMutation({
    mutationFn: (password: string) => resetPassword(name, password),
    onSuccess: () => {
      onNotice({
        kind: "done",
        text: `Replaced the password for ${who}. Its sessions and agent connections are signed out.`,
      });
      void invalidate();
    },
    onError: (error: Error) => {
      onNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const unlink = useMutation({
    mutationFn: () => unlinkUserGithub(name),
    onSuccess: (warning) => {
      onNotice({
        kind: warning ? "problem" : "done",
        text: warning
          ? `Unlinked ${who} from GitHub: ${warning}.`
          : `Unlinked ${who} from GitHub.`,
      });
      void invalidate();
    },
    onError: (error: Error) => {
      onNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const writes = useMutation({
    mutationFn: (list: string[] | null) => setUserWrites(name, list),
    onMutate: () => {
      onNotice(null);
    },
    onSuccess: (stored) => {
      queryClient.setQueryData<AccountDetail | null>(
        userKey(name),
        (old) => old && { ...old, writes: stored },
      );
      onNotice({
        kind: "done",
        text:
          stored === null
            ? `Removed the write list of ${who}.`
            : `Saved the write list of ${who}.`,
      });
    },
    onError: (error: Error) => {
      onNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteUser(name),
    onSuccess: () => {
      onNotice({ kind: "done", text: `Deleted ${who}.` });
      queryClient.removeQueries({ queryKey: userKey(name) });
      void invalidate();
      // Replace, so the back button does not return to the deleted account.
      void navigate(listTo, { replace: true });
    },
    onError: (error: Error) => {
      onNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const shown = account.data;

  return (
    <section
      aria-labelledby="user-detail-heading"
      className="flex min-w-0 flex-col gap-5 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <Link
        to={listTo}
        className={`self-start rounded text-sm text-link underline-offset-2 hover:underline ${FOCUS_RING}`}
      >
        Back to users
      </Link>

      {shown === undefined && account.isPending && (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Loading {name}...
        </p>
      )}

      {account.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(account.error)}
        </p>
      )}

      {shown === null && (
        <>
          <h2
            id="user-detail-heading"
            ref={heading}
            tabIndex={-1}
            className="text-lg font-semibold focus:outline-none"
          >
            No such account
          </h2>
          <p className="text-sm text-slate-500 dark:text-slate-400">
            There is no account named {name}.
          </p>
        </>
      )}

      {shown && (
        <AccountFields
          key={shown.user.name}
          detail={shown}
          heading={heading}
          isSelf={isSelf}
          onPatch={(body) => {
            patch.mutate(body);
          }}
          onReset={(password) => {
            reset.mutate(password);
          }}
          onUnlink={() => {
            unlink.mutate();
          }}
          unlinking={unlink.isPending}
          onWrites={(list) => {
            writes.mutate(list);
          }}
          savingWrites={writes.isPending}
          onDelete={() => {
            remove.mutate();
          }}
        />
      )}
    </section>
  );
}

function AccountFields({
  detail,
  heading,
  isSelf,
  onPatch,
  onReset,
  onUnlink,
  unlinking,
  onWrites,
  savingWrites,
  onDelete,
}: {
  detail: AccountDetail;
  heading: (node: HTMLHeadingElement | null) => void;
  isSelf: boolean;
  onPatch: (body: PatchUserBody) => void;
  onReset: (password: string) => void;
  onUnlink: () => void;
  unlinking: boolean;
  onWrites: (list: string[] | null) => void;
  savingWrites: boolean;
  onDelete: () => void;
}) {
  const account = detail.user;
  // The field follows a change made elsewhere (another tab, a refresh) as
  // long as nothing was typed into it; typed text is never overwritten.
  const [display, setDisplay] = useState(account.display);
  const [displayBase, setDisplayBase] = useState(account.display);
  if (account.display !== displayBase) {
    setDisplayBase(account.display);
    if (display === displayBase) setDisplay(account.display);
  }
  const [password, setPassword] = useState("");
  const [resetting, setResetting] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const deleteTrigger = useRef<HTMLButtonElement>(null);
  const lastSeen = account.last_seen ?? null;

  /** Give up on the pending delete, and hand the focus back to what asked. */
  function abandonDelete() {
    setConfirming(false);
    deleteTrigger.current?.focus();
  }

  return (
    <>
      <header>
        <h2
          id="user-detail-heading"
          ref={heading}
          tabIndex={-1}
          className="text-lg font-semibold focus:outline-none"
        >
          {account.display}
        </h2>
        <p className="font-mono text-sm text-slate-500 dark:text-slate-400">
          {account.name}
        </p>
      </header>

      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
        <dt className={TERM}>Sign-in</dt>
        <dd>{signInWords(account.sign_in)}</dd>
        <dt className={TERM}>Last seen</dt>
        <dd className="tabular-nums">
          {lastSeen === null ? "Never" : formatDay(lastSeen)}
        </dd>
        {account.email && (
          <>
            <dt className={TERM}>Email</dt>
            <dd className="break-all">{account.email}</dd>
          </>
        )}
      </dl>

      <form
        className="flex flex-col gap-1"
        onSubmit={(event) => {
          event.preventDefault();
          onPatch({ display });
        }}
      >
        <span className={TERM}>Display name</span>
        <div className="flex items-center gap-2">
          <input
            id={`display-${account.name}`}
            aria-label={`Display name for ${account.name}`}
            value={display}
            onChange={(event) => {
              setDisplay(event.target.value);
            }}
            className={`min-w-0 flex-1 ${FIELD}`}
          />
          <button
            type="submit"
            aria-label={`Save display for ${account.name}`}
            disabled={display === account.display}
            className={SECONDARY}
          >
            Save
          </button>
        </div>
      </form>

      <div className="flex flex-col gap-1">
        <span className={TERM}>Role</span>
        <select
          id={`role-${account.name}`}
          aria-label={`Role for ${account.name}`}
          value={account.role}
          onChange={(event) => {
            const role = event.target.value as Role;
            const reflection = raiseForNone(role, account.reflection);
            onPatch(
              reflection === account.reflection
                ? { role }
                : { role, reflection },
            );
          }}
          className={FIELD}
        >
          {ROLES.map((role) => (
            <option key={role} value={role}>
              {role}
            </option>
          ))}
        </select>
      </div>

      <div className="flex flex-col gap-1">
        <span className={TERM}>Reflection</span>
        <ReflectionLevelSelect
          label={`Reflection for ${account.name}`}
          knowledge={account.role}
          value={account.reflection}
          onChange={(reflection) => {
            onPatch({ reflection });
          }}
        />
      </div>

      <div className="flex flex-col gap-1">
        <span className={TERM}>State</span>
        <div className={`flex ${CONTROL_HEIGHT} items-center gap-3 text-sm`}>
          <span>{account.disabled ? "Disabled" : "Active"}</span>
          {!isSelf && (
            <button
              type="button"
              aria-label={`${account.disabled ? "Reactivate" : "Deactivate"} ${account.name}`}
              onClick={() => {
                onPatch({ disabled: !account.disabled });
              }}
              className={SECONDARY}
            >
              {account.disabled ? "Reactivate" : "Deactivate"}
            </button>
          )}
        </div>
      </div>

      <div className="flex flex-col gap-1">
        <span className={TERM}>Password</span>
        {resetting ? (
          <form
            className="flex flex-wrap items-center gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              onReset(password);
              setPassword("");
              setResetting(false);
            }}
          >
            <label className="sr-only" htmlFor={`password-${account.name}`}>
              New password for {account.name}
            </label>
            <input
              id={`password-${account.name}`}
              type="password"
              autoComplete="new-password"
              required
              autoFocus
              value={password}
              onChange={(event) => {
                setPassword(event.target.value);
              }}
              className={`min-w-0 flex-1 ${FIELD}`}
            />
            <button
              type="submit"
              aria-label={`Set password for ${account.name}`}
              className={SECONDARY}
            >
              Set
            </button>
            <button
              type="button"
              aria-label={`Cancel the password reset for ${account.name}`}
              onClick={() => {
                setPassword("");
                setResetting(false);
              }}
              className={SECONDARY}
            >
              Cancel
            </button>
          </form>
        ) : (
          <button
            type="button"
            onClick={() => {
              setResetting(true);
            }}
            className={`self-start ${SECONDARY}`}
          >
            Reset password
          </button>
        )}
      </div>

      <div className="flex flex-col gap-1">
        <span className={TERM}>GitHub</span>
        <div className={`flex ${CONTROL_HEIGHT} items-center gap-3 text-sm`}>
          <span>{account.github_linked ? "Linked" : "Not linked"}</span>
          {account.github_linked && (
            <button
              type="button"
              aria-label={`Unlink ${account.name} from GitHub`}
              disabled={unlinking}
              onClick={onUnlink}
              className={SECONDARY}
            >
              Unlink
            </button>
          )}
        </div>
      </div>

      <WriteList
        name={account.name}
        loaded={detail.loaded}
        stored={detail.writes}
        saving={savingWrites}
        onSave={onWrites}
      />

      <Drafts loaded={detail.loaded} drafts={detail.drafts} />

      {/*
        Two steps rather than a browser confirm: a dialog the browser owns
        cannot be reached by a test, cannot be styled and cannot be dismissed
        by the keyboard the way the rest of this screen can.
      */}
      <div
        className="flex flex-wrap items-center gap-2 border-t border-slate-200 pt-4 dark:border-slate-800"
        onKeyDown={(event) => {
          if (event.key === "Escape" && confirming) {
            event.stopPropagation();
            abandonDelete();
          }
        }}
        onBlur={(event) => {
          // Only when the focus really landed somewhere else: a `focusout`
          // with no destination is what a click looks like mid-flight, and
          // closing there would eat the second click this exists to require.
          if (
            confirming &&
            focusLeft(event.currentTarget, event.relatedTarget)
          ) {
            setConfirming(false);
          }
        }}
      >
        <button
          ref={deleteTrigger}
          type="button"
          aria-label={`Delete ${account.name}`}
          aria-expanded={confirming}
          onClick={() => {
            setConfirming(true);
          }}
          className={DANGER}
        >
          Delete
        </button>
        {confirming && (
          <>
            <button
              type="button"
              aria-label={`Confirm delete of ${account.name}`}
              autoFocus
              onClick={() => {
                setConfirming(false);
                onDelete();
              }}
              className={DANGER}
            >
              Confirm delete
            </button>
            <button
              type="button"
              aria-label={`Keep ${account.name}`}
              onClick={abandonDelete}
              className={SECONDARY}
            >
              Keep
            </button>
          </>
        )}
      </div>
    </>
  );
}

/** The entries typed into the write list field, split at commas. */
function writeEntries(text: string): string[] {
  return text
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry !== "");
}

/**
 * The domains the account may write to. No list means it writes wherever its
 * role and memberships allow; a list only lowers that. Typed as entries
 * separated by commas, each a domain name or a name prefix ending in `*`;
 * the server checks them as `crystalline users writes` does.
 */
function WriteList({
  name,
  loaded,
  stored,
  saving,
  onSave,
}: {
  name: string;
  loaded: boolean;
  stored: string[] | null;
  saving: boolean;
  onSave: (list: string[] | null) => void;
}) {
  const storedText = (stored ?? []).join(", ");
  const [text, setText] = useState(storedText);
  const [base, setBase] = useState(storedText);
  // A list changed elsewhere replaces the field while nothing is typed in
  // it, and a save shows the list as stored (sorted, repeats gone).
  if (storedText !== base) {
    setBase(storedText);
    const typed = [...new Set(writeEntries(text))].sort().join(", ");
    if (text === base || typed === [...(stored ?? [])].sort().join(", ")) {
      setText(storedText);
    }
  }
  const entries = writeEntries(text);
  const unchanged =
    [...new Set(entries)].sort().join(", ") ===
    [...(stored ?? [])].sort().join(", ");

  return (
    <form
      className="flex flex-col gap-1"
      onSubmit={(event) => {
        event.preventDefault();
        onSave(entries.length === 0 ? null : entries);
      }}
    >
      <span className={TERM}>Write list</span>
      {!loaded ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">Loading...</p>
      ) : (
        <>
          <p className="text-sm text-slate-600 dark:text-slate-300">
            {stored === null
              ? "No list: it writes wherever its role and memberships allow."
              : "It writes only to these domains."}
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <input
              aria-label={`Write list for ${name}`}
              value={text}
              autoComplete="off"
              placeholder="eng, agents-*"
              onChange={(event) => {
                setText(event.target.value);
              }}
              className={`min-w-0 flex-1 font-mono ${FIELD}`}
            />
            <button
              type="submit"
              aria-label={`Save write list for ${name}`}
              disabled={saving || unchanged}
              className={SECONDARY}
            >
              Save
            </button>
            {stored !== null && (
              <button
                type="button"
                aria-label={`Remove the write list of ${name}`}
                disabled={saving}
                onClick={() => {
                  onSave(null);
                }}
                className={SECONDARY}
              >
                Remove
              </button>
            )}
          </div>
        </>
      )}
    </form>
  );
}

/**
 * What the account holds in drafts, counted per domain. Read only: the
 * drafts are the person's own.
 */
function Drafts({
  loaded,
  drafts,
}: {
  loaded: boolean;
  drafts: DraftCount[] | null;
}) {
  return (
    <div className="flex flex-col gap-1 text-sm">
      <span className={TERM}>Drafts</span>
      {!loaded ? (
        <p className="text-slate-500 dark:text-slate-400">Loading...</p>
      ) : drafts === null ? (
        <p className="text-slate-600 dark:text-slate-300">
          The drafts could not be read.
        </p>
      ) : drafts.length === 0 ? (
        <p className="text-slate-600 dark:text-slate-300">No drafts.</p>
      ) : (
        <ul className="flex flex-col gap-0.5">
          {drafts.map((row) => (
            <li key={row.domain}>
              <span className="font-mono">{row.domain}</span>:{" "}
              {row.count === 1 ? "1 draft" : `${String(row.count)} drafts`}
            </li>
          ))}
        </ul>
      )}
      <p className="text-slate-500 dark:text-slate-400">
        Drafts belong to the person, so they are only counted here. A delete
        keeps them under the name; end them afterwards on the list.
      </p>
    </div>
  );
}

/**
 * The optimistic copy of an account with a patch applied.
 *
 * Field by field rather than a spread, because the patch body says "leave this
 * alone" with an absent or null field and an account says nothing with null:
 * spreading one into the other would blank a role the admin never touched.
 */
function withPatch(account: User, body: PatchUserBody): User {
  const next: User = { ...account };
  if (body.role !== undefined && body.role !== null) {
    next.role = body.role;
  }
  if (body.reflection !== undefined && body.reflection !== null) {
    next.reflection = body.reflection;
  }
  if (body.display !== undefined && body.display !== null) {
    next.display = body.display;
  }
  if (body.disabled !== undefined && body.disabled !== null) {
    next.disabled = body.disabled;
  }
  return next;
}
