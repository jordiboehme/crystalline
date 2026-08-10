/**
 * Account administration, the admin-only screen: who may sign in, what each
 * account may do, and the levers an admin has - create, role, display name,
 * deactivate/reactivate, password reset, delete. Every refusal is the
 * server's own sentence (NOT_LAST_ADMIN above all), shown where the action
 * happened; the force escape hatches exist only in the CLI, on purpose.
 *
 * Nothing here asks the server whether an action is allowed before offering
 * it. The server decides, every time, and this screen is built to carry the
 * answer back rather than to predict it: the one thing it declines to draw is
 * the caller's own deactivate control, which is not a guess about the rules
 * but a refusal to offer somebody the door key to lock themselves out with.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useRef, useState } from "react";

import { problemDetail } from "../api/client";
import type { PatchUserBody, Role, User } from "../api/model";
import {
  USERS_QUERY_KEY,
  createUser,
  deleteUser,
  fetchUsers,
  patchUser,
  resetPassword,
} from "../api/users";
import { useAuth } from "../auth/AuthContext";
import { formatDay } from "../format";
import NotFound from "./NotFound";

/** The roles an account can hold, least privileged first, as the API orders them. */
const ROLES: Role[] = ["viewer", "editor", "admin"];

/** What the screen is currently saying, and whether it is bad news. */
interface Notice {
  kind: "problem" | "done";
  text: string;
}

const FIELD_CLASSES =
  "rounded border border-slate-300 bg-white px-2 py-1 text-sm outline-none focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 dark:border-slate-700 dark:bg-slate-900";

const BUTTON_CLASSES =
  "rounded border border-slate-300 px-2 py-1 text-sm hover:bg-slate-100 focus-visible:ring-2 focus-visible:ring-accent-600 dark:focus-visible:ring-accent-400 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-50 dark:border-slate-700 dark:hover:bg-slate-800";

const DANGER_CLASSES =
  "rounded border border-red-300 px-2 py-1 text-sm text-red-800 hover:bg-red-50 focus-visible:ring-2 focus-visible:ring-red-500 focus-visible:outline-none dark:border-red-900 dark:text-red-200 dark:hover:bg-red-950";

export default function UsersAdmin() {
  const { capabilities } = useAuth();
  if (!capabilities.canAdminister) {
    return <NotFound />;
  }
  return <UsersPanel />;
}

function UsersPanel() {
  const queryClient = useQueryClient();
  const { user } = useAuth();
  const users = useQuery({ queryKey: USERS_QUERY_KEY, queryFn: fetchUsers });
  const [notice, setNotice] = useState<Notice | null>(null);

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: USERS_QUERY_KEY });

  const patch = useMutation({
    mutationFn: ({ name, body }: { name: string; body: PatchUserBody }) =>
      patchUser(name, body),
    // Optimistic for the non-content edits the quality bar names (role,
    // display, state): apply, roll back on refusal, resettle from the server.
    onMutate: async ({ name, body }) => {
      await queryClient.cancelQueries({ queryKey: USERS_QUERY_KEY });
      const before = queryClient.getQueryData<User[]>(USERS_QUERY_KEY);
      queryClient.setQueryData<User[]>(USERS_QUERY_KEY, (old) =>
        (old ?? []).map((account) =>
          account.name === name ? withPatch(account, body) : account,
        ),
      );
      setNotice(null);
      return { before };
    },
    onError: (error: Error, _vars, context) => {
      if (context?.before) {
        queryClient.setQueryData(USERS_QUERY_KEY, context.before);
      }
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
    onSettled: () => void invalidate(),
  });

  const remove = useMutation({
    mutationFn: deleteUser,
    onSuccess: (_result, name) => {
      setNotice({ kind: "done", text: `Deleted ${name}.` });
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const reset = useMutation({
    mutationFn: ({ name, password }: { name: string; password: string }) =>
      resetPassword(name, password),
    onSuccess: (_result, { name }) => {
      setNotice({
        kind: "done",
        text: `Replaced the password for ${name}. Every session it held is signed out.`,
      });
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const add = useMutation({
    mutationFn: createUser,
    onSuccess: (created) => {
      setNotice({ kind: "done", text: `Added ${created.name}.` });
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const accounts = users.data ?? [];

  return (
    <div className="flex flex-col gap-6">
      <header>
        <h1 className="text-xl font-semibold">Users</h1>
        <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
          Who may sign in to this instance, and what each of them may do. The
          server decides every one of these changes; whatever it refuses is said
          here in its own words.
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

      {users.isPending && <AccountsSkeleton />}

      {users.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(users.error)}
        </p>
      )}

      {!users.isPending && !users.error && accounts.length === 0 && (
        <p className="rounded border border-dashed border-slate-300 px-3 py-6 text-sm text-slate-500 dark:border-slate-700 dark:text-slate-400">
          No accounts yet. Add one below, or let a trusted-header proxy
          provision them as people arrive through it.
        </p>
      )}

      {accounts.length > 0 && (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <caption className="sr-only">
              Every account on this instance
            </caption>
            <thead className="text-xs tracking-wide text-slate-500 uppercase dark:text-slate-400">
              <tr>
                <th scope="col" className="px-2 py-2">
                  Name
                </th>
                <th scope="col" className="px-2 py-2">
                  Display
                </th>
                <th scope="col" className="px-2 py-2">
                  Role
                </th>
                <th scope="col" className="px-2 py-2">
                  State
                </th>
                <th scope="col" className="px-2 py-2">
                  Last seen
                </th>
                <th scope="col" className="px-2 py-2">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
              {accounts.map((account) => (
                <AccountRow
                  key={account.name}
                  account={account}
                  isSelf={account.name === user?.name}
                  onPatch={(body) => {
                    patch.mutate({ name: account.name, body });
                  }}
                  onReset={(password) => {
                    reset.mutate({ name: account.name, password });
                  }}
                  onDelete={() => {
                    remove.mutate(account.name);
                  }}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}

      <CreateForm
        pending={add.isPending}
        onCreate={(body) => add.mutateAsync(body)}
      />
    </div>
  );
}

/**
 * One account, and every lever an admin has on it.
 *
 * The row holds its own draft state - the display name being typed, the
 * password being set, whether a delete has been asked for once - because those
 * are about this row and nothing else on the screen needs to know them. It is
 * keyed by login name above, so a refetch that reorders the listing does not
 * carry a half-typed name onto somebody else's row.
 */
function AccountRow({
  account,
  isSelf,
  onPatch,
  onReset,
  onDelete,
}: {
  account: User;
  isSelf: boolean;
  onPatch: (body: PatchUserBody) => void;
  onReset: (password: string) => void;
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
    <tr className="align-top">
      {/*
        The login name is the row's header, not a cell like the others: it is
        what every control in the row is about, and marking it up as one is
        what lets a screen reader say whose row a bare "Reset password" is on.
      */}
      <th scope="row" className="px-2 py-2 font-mono font-normal">
        {account.name}
      </th>

      <td className="px-2 py-2">
        <form
          className="flex items-center gap-1"
          onSubmit={(event) => {
            event.preventDefault();
            onPatch({ display });
          }}
        >
          <label className="sr-only" htmlFor={`display-${account.name}`}>
            Display name for {account.name}
          </label>
          <input
            id={`display-${account.name}`}
            value={display}
            onChange={(event) => {
              setDisplay(event.target.value);
            }}
            className={`w-32 ${FIELD_CLASSES}`}
          />
          <button
            type="submit"
            aria-label={`Save display for ${account.name}`}
            disabled={display === account.display}
            className={BUTTON_CLASSES}
          >
            Save
          </button>
        </form>
      </td>

      <td className="px-2 py-2">
        <label className="sr-only" htmlFor={`role-${account.name}`}>
          Role for {account.name}
        </label>
        <select
          id={`role-${account.name}`}
          value={account.role}
          onChange={(event) => {
            onPatch({ role: event.target.value as Role });
          }}
          className={FIELD_CLASSES}
        >
          {ROLES.map((role) => (
            <option key={role} value={role}>
              {role}
            </option>
          ))}
        </select>
      </td>

      <td className="px-2 py-2">
        <div className="flex items-center gap-2">
          <span>{account.disabled ? "Disabled" : "Active"}</span>
          {/*
            Not offered on the caller's own row. The server refuses it anyway;
            this only declines to hold the door open for somebody about to
            lock themselves out of their own instance.
          */}
          {!isSelf && (
            <button
              type="button"
              aria-label={`${account.disabled ? "Reactivate" : "Deactivate"} ${account.name}`}
              onClick={() => {
                onPatch({ disabled: !account.disabled });
              }}
              className={BUTTON_CLASSES}
            >
              {account.disabled ? "Reactivate" : "Deactivate"}
            </button>
          )}
        </div>
      </td>

      <td className="px-2 py-2 tabular-nums">
        {lastSeen === null ? (
          <span className="text-slate-500 dark:text-slate-400">Never</span>
        ) : (
          formatDay(lastSeen)
        )}
      </td>

      <td className="px-2 py-2">
        <div className="flex flex-wrap items-center gap-2">
          {resetting ? (
            <form
              className="flex items-center gap-1"
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
                className={`w-36 ${FIELD_CLASSES}`}
              />
              <button
                type="submit"
                aria-label={`Set password for ${account.name}`}
                className={BUTTON_CLASSES}
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
                className={BUTTON_CLASSES}
              >
                Cancel
              </button>
            </form>
          ) : (
            /*
              Named by its own text rather than an `aria-label` naming the
              account: the row header beside it already says whose row this
              is, and a label reading "reset password for eddy" would also
              answer a query for the password field, which is a different
              control on a different form.
            */
            <button
              type="button"
              onClick={() => {
                setResetting(true);
              }}
              className={BUTTON_CLASSES}
            >
              Reset password
            </button>
          )}

          {/*
            Two steps rather than a browser confirm: a dialog the browser owns
            cannot be reached by a test, cannot be styled and cannot be
            dismissed by the keyboard the way the rest of this screen can.
          */}
          <div
            className="flex items-center gap-2"
            onKeyDown={(event) => {
              if (event.key === "Escape" && confirming) {
                event.stopPropagation();
                abandonDelete();
              }
            }}
            onBlur={(event) => {
              // Only when the focus actually landed somewhere else. A
              // `focusout` with no destination is what a click looks like
              // mid-flight, and taking the confirmation away there would eat
              // the second click this exists to require.
              const next = event.relatedTarget;
              if (
                confirming &&
                next instanceof Node &&
                !event.currentTarget.contains(next)
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
              className={DANGER_CLASSES}
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
                  className={DANGER_CLASSES}
                >
                  Confirm delete
                </button>
                <button
                  type="button"
                  aria-label={`Keep ${account.name}`}
                  onClick={abandonDelete}
                  className={BUTTON_CLASSES}
                >
                  Keep
                </button>
              </>
            )}
          </div>
        </div>
      </td>
    </tr>
  );
}

/**
 * Add an account. The password is the initial one; the account can be given a
 * new one from its row.
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
  const passwordField = useId();
  const [name, setName] = useState("");
  const [display, setDisplay] = useState("");
  const [role, setRole] = useState<Role>("viewer");
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
            ...(display === "" ? {} : { display }),
          }).then(
            () => {
              setName("");
              setDisplay("");
              setRole("viewer");
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
            className={FIELD_CLASSES}
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
            className={FIELD_CLASSES}
          />
        </div>

        <div className="flex flex-col gap-1">
          <label
            htmlFor={roleField}
            className="text-xs text-slate-500 dark:text-slate-400"
          >
            Role for the new account
          </label>
          <select
            id={roleField}
            value={role}
            onChange={(event) => {
              setRole(event.target.value as Role);
            }}
            className={FIELD_CLASSES}
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
            className={FIELD_CLASSES}
          />
        </div>

        <button
          type="submit"
          disabled={pending}
          className="rounded bg-sky-600 px-3 py-1.5 text-sm font-medium text-white hover:bg-sky-500 focus-visible:ring-2 focus-visible:ring-sky-400 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-60"
        >
          Add user
        </button>
      </form>
    </section>
  );
}

/**
 * The shape of the table, while the table is on its way.
 *
 * A skeleton rather than a spinner: the listing is the screen, so what it
 * costs nothing to promise is where the rows will be. It is hidden from the
 * accessibility tree and named by the live region beside it instead, because
 * six grey bars are not something to read out.
 */
function AccountsSkeleton() {
  return (
    <div role="status" aria-busy="true" aria-label="Loading accounts">
      <div aria-hidden="true" className="flex animate-pulse flex-col gap-2">
        {[0, 1, 2].map((row) => (
          <div
            key={row}
            className="h-8 rounded bg-slate-100 dark:bg-slate-800"
          />
        ))}
      </div>
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
  if (body.display !== undefined && body.display !== null) {
    next.display = body.display;
  }
  if (body.disabled !== undefined && body.disabled !== null) {
    next.disabled = body.disabled;
  }
  return next;
}
