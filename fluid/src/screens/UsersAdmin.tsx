/**
 * Account administration, the admin-only screen: who may sign in, what each
 * account may do, and the levers an admin has - create, role, display name,
 * deactivate/reactivate, password reset, delete, and ending a removed account's
 * drafts. Every refusal is the
 * server's own sentence (NOT_LAST_ADMIN above all), shown where the action
 * happened; the force escape hatches exist only in the CLI, on purpose.
 *
 * Nothing here asks the server whether an action is allowed before offering
 * it. The server decides, every time, and this screen is built to carry the
 * answer back rather than to predict it: the one thing it declines to draw is
 * the caller's own deactivate control, which is not a guess about the rules
 * but a refusal to offer somebody the door key to lock themselves out with.
 */

import { usePageTitle } from "../usePageTitle";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useRef, useState } from "react";

import { problemDetail } from "../api/client";
import type { PatchUserBody, ReflectionLevel, Role, User } from "../api/model";
import type { DiscardedDrafts } from "../api/users";
import {
  USERS_QUERY_KEY,
  createUser,
  deleteUser,
  discardUserDrafts,
  fetchUsers,
  patchUser,
  resetPassword,
} from "../api/users";
import { useAuth } from "../auth/AuthContext";
import { Loading } from "../components/Loading";
import { BUTTON, CONTROL_HEIGHT, FIELD } from "../components/primitives";
import { ReflectionLevelSelect } from "../components/ReflectionLevelSelect";
import { raiseForNone, reflectionAfter } from "../components/reflectionLevels";
import { formatDay } from "../format";
import NotFound from "./NotFound";
import { refreshAfterWrite } from "../query/refreshAfterWrite";

/** The roles an account can hold, least privileged first, as the API orders them. */
const ROLES: Role[] = ["none", "viewer", "editor", "admin"];

/** What the screen is currently saying, and whether it is bad news. */
interface Notice {
  kind: "problem" | "done";
  text: string;
}

/**
 * The row's own buttons, from the shared tiers rather than hand-rolled here.
 *
 * Secondary throughout: every one of them acts on the account whose row it
 * sits on, and none of them is the thing an admin came to this screen to do -
 * that is "Add user" at the bottom, which is the only primary on the screen.
 * The disabled Save reads as waiting for a change rather than as broken,
 * because the tier says so in the same words everywhere else in the app.
 */
const ROW_BUTTON = `${CONTROL_HEIGHT} ${BUTTON.secondary}`;

const DANGER_BUTTON = `${CONTROL_HEIGHT} ${BUTTON.destructive}`;

export default function UsersAdmin() {
  const { capabilities } = useAuth();
  usePageTitle(capabilities.canAdminister ? "Users" : null);
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
    refreshAfterWrite(queryClient, { queryKey: USERS_QUERY_KEY });

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

  const accounts = users.data ?? [];

  return (
    <div className="flex flex-col gap-6">
      <header>
        <h1 className="text-display">Users</h1>
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
            {/*
              Sentence case: these are six short words a reader glances at
              once, and setting them in capitals made the quietest row on the
              screen the loudest thing on it.
            */}
            <thead className="text-caption font-semibold text-slate-500 dark:text-slate-400">
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
                  Reflection
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

      <EndDraftsForm
        pending={endDrafts.isPending}
        onEnd={(name) => endDrafts.mutateAsync(name)}
      />
    </div>
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
      <th scope="row" className="px-2 py-2 font-normal">
        {/*
          Not a control, and still on the row's one line: the login name is
          what every control beside it is about, so it stands at their height
          rather than at the top of the cell. The mono face stays - a login
          name is an identifier, and this column is a column of them.
        */}
        <span className={`flex ${CONTROL_HEIGHT} items-center font-mono`}>
          {account.name}
        </span>
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
            className={`w-32 ${FIELD}`}
          />
          <button
            type="submit"
            aria-label={`Save display for ${account.name}`}
            disabled={display === account.display}
            className={ROW_BUTTON}
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
      </td>

      <td className="px-2 py-2">
        <ReflectionLevelSelect
          label={`Reflection for ${account.name}`}
          knowledge={account.role}
          value={account.reflection}
          onChange={(reflection) => {
            onPatch({ reflection });
          }}
        />
      </td>

      <td className="px-2 py-2">
        <div className={`flex ${CONTROL_HEIGHT} items-center gap-2`}>
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
              className={ROW_BUTTON}
            >
              {account.disabled ? "Reactivate" : "Deactivate"}
            </button>
          )}
        </div>
      </td>

      <td className="px-2 py-2 tabular-nums">
        <span className={`flex ${CONTROL_HEIGHT} items-center`}>
          {lastSeen === null ? (
            <span className="text-slate-500 dark:text-slate-400">Never</span>
          ) : (
            formatDay(lastSeen)
          )}
        </span>
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
                className={`w-36 ${FIELD}`}
              />
              <button
                type="submit"
                aria-label={`Set password for ${account.name}`}
                className={ROW_BUTTON}
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
                className={ROW_BUTTON}
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
              className={ROW_BUTTON}
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
              className={DANGER_BUTTON}
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
                  className={DANGER_BUTTON}
                >
                  Confirm delete
                </button>
                <button
                  type="button"
                  aria-label={`Keep ${account.name}`}
                  onClick={abandonDelete}
                  className={ROW_BUTTON}
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
