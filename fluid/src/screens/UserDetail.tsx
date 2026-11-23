/**
 * One account and every lever an admin has on it: display name, role,
 * Reflection level, state, password reset, GitHub link and delete.
 *
 * It has its own address, `/admin/users/<name>`, so it can be linked and the
 * back button closes it. Beside the table at wide widths, the whole page at
 * narrow ones; the list screen decides which.
 *
 * There is no route that reads one account, so the panel finds it with a
 * search by its name (see `fetchUser`). The row the table already holds is
 * shown while that read runs, so opening a row draws at once.
 *
 * Nothing here asks the server whether an action is allowed before offering
 * it; the server decides and its sentence comes back through `onNotice`. The
 * one control left out is the caller's own deactivate, which is not a guess
 * about the rules but a refusal to offer somebody the key to lock themselves
 * out with.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { Link, useNavigate } from "react-router";

import { problemDetail } from "../api/client";
import type { PatchUserBody, Role, User } from "../api/model";
import type { UsersPage } from "../api/users";
import {
  USERS_QUERY_KEY,
  deleteUser,
  fetchUser,
  patchUser,
  resetPassword,
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

const SECONDARY = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.secondary}`;
const DANGER = `${CONTROL_HEIGHT} whitespace-nowrap ${BUTTON.destructive}`;
const TERM = "text-xs text-slate-500 dark:text-slate-400";

export function UserDetail({
  name,
  fromList,
  isSelf,
  listTo,
  onNotice,
}: {
  name: string;
  /** The row the table holds for this name, shown while the panel reads it. */
  fromList: User | undefined;
  isSelf: boolean;
  /** The list's address, with its search, filters and page. */
  listTo: string;
  onNotice: (notice: Notice | null) => void;
}) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const account = useQuery({
    queryKey: userKey(name),
    queryFn: () => fetchUser(name),
    placeholderData: () => fromList,
  });

  const invalidate = () =>
    refreshAfterWrite(queryClient, { queryKey: USERS_QUERY_KEY });

  const patch = useMutation({
    mutationFn: (body: PatchUserBody) => patchUser(name, body),
    // Optimistic for role, display, Reflection and state: apply to the panel
    // and to every cached page, roll back on refusal, resettle from the server.
    onMutate: async (body) => {
      await queryClient.cancelQueries({ queryKey: USERS_QUERY_KEY });
      const one = queryClient.getQueryData<User | null>(userKey(name));
      const pages = queryClient.getQueriesData<UsersPage>({
        queryKey: [...USERS_QUERY_KEY, "page"],
      });
      const current = one ?? account.data;
      if (current) {
        queryClient.setQueryData(userKey(name), withPatch(current, body));
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
        queryClient.setQueryData(userKey(name), context.one);
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
        text: `Replaced the password for ${name}. Its sessions and agent connections are signed out.`,
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
          ? `Unlinked ${name} from GitHub: ${warning}.`
          : `Unlinked ${name} from GitHub.`,
      });
      void invalidate();
    },
    onError: (error: Error) => {
      onNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteUser(name),
    onSuccess: () => {
      onNotice({ kind: "done", text: `Deleted ${name}.` });
      queryClient.removeQueries({ queryKey: userKey(name) });
      void invalidate();
      void navigate(listTo);
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
          <h2 id="user-detail-heading" className="text-lg font-semibold">
            No such account
          </h2>
          <p className="text-sm text-slate-500 dark:text-slate-400">
            There is no account named {name}.
          </p>
        </>
      )}

      {shown && (
        <AccountFields
          key={shown.name}
          account={shown}
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
          onDelete={() => {
            remove.mutate();
          }}
        />
      )}
    </section>
  );
}

function AccountFields({
  account,
  isSelf,
  onPatch,
  onReset,
  onUnlink,
  unlinking,
  onDelete,
}: {
  account: User;
  isSelf: boolean;
  onPatch: (body: PatchUserBody) => void;
  onReset: (password: string) => void;
  onUnlink: () => void;
  unlinking: boolean;
  onDelete: () => void;
}) {
  const [display, setDisplay] = useState(account.display);
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
        <h2 id="user-detail-heading" className="text-lg font-semibold">
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

      <div className="flex flex-col gap-1 text-sm">
        <span className={TERM}>Write list</span>
        <p className="text-slate-600 dark:text-slate-300">
          The domains this account may write to are set on the server with{" "}
          <code className="font-mono">
            crystalline users writes {account.name}
          </code>
          .
        </p>
      </div>

      <div className="flex flex-col gap-1 text-sm">
        <span className={TERM}>Drafts</span>
        <p className="text-slate-600 dark:text-slate-300">
          A delete keeps the account&apos;s drafts under its name. End them
          afterwards under &ldquo;End a removed account&apos;s drafts&rdquo; on
          the list.
        </p>
      </div>

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
