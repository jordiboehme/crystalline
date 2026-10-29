/**
 * The Reflection collaborators of one domain kept as files: accounts given
 * the Reflection `reader` or `contributor` level here, beside whatever their
 * engram access gives them. A grant opens no engram.
 *
 * Drawn only for a caller who manages the domain, because only that caller
 * receives the list (`reflection_collaborators` is null for everybody else
 * and on a domain kept in the database). No label here contains the invite
 * form's names ("Account", "Level"), so a substring match on those never
 * finds two controls.
 */

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { ReactElement } from "react";
import { useId, useState } from "react";

import { problemDetail } from "../api/client";
import {
  membersKey,
  removeReflectionCollaborator,
  setReflectionCollaborator,
} from "../api/members";
import type { ReflectionGrant } from "../api/model";
import { formatDay } from "../format";
import { DestructiveAction } from "./DestructiveAction";
import { BUTTON, FIELD, Field } from "./primitives";

type GrantLevel = "reader" | "contributor";

const GRANT_LEVELS: GrantLevel[] = ["reader", "contributor"];

export function ReflectionCollaborators({
  domain,
  grants,
  disabledReason,
}: {
  domain: string;
  grants: ReflectionGrant[];
  disabledReason?: string | undefined;
}): ReactElement {
  const queryClient = useQueryClient();
  const headingId = useId();
  const [problem, setProblem] = useState<string | null>(null);
  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: membersKey(domain) });

  const grant = useMutation({
    mutationFn: ({ account, level }: { account: string; level: GrantLevel }) =>
      setReflectionCollaborator(domain, account, level),
    onSuccess: () => {
      setProblem(null);
      void invalidate();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });
  const remove = useMutation({
    mutationFn: (account: string) =>
      removeReflectionCollaborator(domain, account),
    onSuccess: () => {
      setProblem(null);
      void invalidate();
    },
    onError: (error: Error) => {
      setProblem(problemDetail(error));
    },
  });

  return (
    <section aria-labelledby={headingId} className="flex flex-col gap-2">
      <h3 id={headingId} className="text-sm font-semibold">
        Reflection collaborators
      </h3>
      <p className="text-sm text-ink-3">
        People who read or edit this domain&apos;s Reflection without reading
        its engrams.
      </p>
      {problem !== null && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problem}
        </p>
      )}
      {grants.length === 0 ? (
        <p className="text-sm text-slate-500 dark:text-slate-400">
          No Reflection collaborators yet.
        </p>
      ) : (
        <div className="overflow-x-auto">
          <table
            aria-labelledby={headingId}
            className="w-full text-left text-sm"
          >
            <thead className="text-caption font-semibold text-slate-500 dark:text-slate-400">
              <tr>
                <th scope="col" className="px-2 py-2">
                  Account
                </th>
                <th scope="col" className="px-2 py-2">
                  Reflection
                </th>
                <th scope="col" className="px-2 py-2">
                  Added
                </th>
                <th scope="col" className="px-2 py-2">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
              {grants.map((row) => (
                <GrantRow
                  key={row.account}
                  row={row}
                  pending={
                    (grant.isPending &&
                      grant.variables?.account === row.account) ||
                    (remove.isPending && remove.variables === row.account)
                  }
                  disabledReason={disabledReason}
                  onLevel={(level) => {
                    setProblem(null);
                    grant.mutate({ account: row.account, level });
                  }}
                  onRemove={() => {
                    setProblem(null);
                    remove.mutate(row.account);
                  }}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
      <AddCollaborator
        pending={grant.isPending}
        disabledReason={disabledReason}
        onAdd={(account, level) => {
          setProblem(null);
          return grant.mutateAsync({ account, level });
        }}
      />
    </section>
  );
}

function GrantRow({
  row,
  pending,
  disabledReason,
  onLevel,
  onRemove,
}: {
  row: ReflectionGrant;
  pending: boolean;
  disabledReason?: string | undefined;
  onLevel: (level: GrantLevel) => void;
  onRemove: () => void;
}): ReactElement {
  const selectId = useId();
  return (
    <tr className="align-top">
      <th scope="row" className="px-2 py-2 font-normal">
        {row.account}
      </th>
      <td className="px-2 py-2">
        <label htmlFor={selectId} className="sr-only">
          {`Reflection access for ${row.account}`}
        </label>
        <select
          id={selectId}
          value={row.level}
          disabled={pending || disabledReason !== undefined}
          onChange={(event) => {
            onLevel(event.target.value as GrantLevel);
          }}
          className={FIELD}
        >
          {GRANT_LEVELS.map((level) => (
            <option key={level} value={level}>
              {level}
            </option>
          ))}
        </select>
      </td>
      <td className="px-2 py-2 tabular-nums">{formatDay(row.added_at)}</td>
      <td className="px-2 py-2">
        <DestructiveAction
          label="Remove"
          confirmLabel="Confirm remove"
          ariaLabel={`Remove ${row.account}`}
          confirmAriaLabel={`Confirm remove ${row.account}`}
          pending={pending}
          disabledReason={disabledReason}
          onConfirm={onRemove}
        />
      </td>
    </tr>
  );
}

function AddCollaborator({
  pending,
  disabledReason,
  onAdd,
}: {
  pending: boolean;
  disabledReason?: string | undefined;
  onAdd: (account: string, level: GrantLevel) => Promise<unknown>;
}): ReactElement {
  const [account, setAccount] = useState("");
  const [level, setLevel] = useState<GrantLevel>("reader");
  const nameField = useId();
  const levelField = useId();
  const disabled = pending || disabledReason !== undefined;
  return (
    <form
      className="flex flex-wrap items-end gap-3 pt-2"
      onSubmit={(event) => {
        event.preventDefault();
        const trimmed = account.trim();
        if (trimmed === "" || disabled) {
          return;
        }
        onAdd(trimmed, level)
          .then(() => {
            setAccount("");
          })
          .catch(() => {
            // Refused: the section shows the server's words, and the typed
            // name stays so it can be fixed.
          });
      }}
    >
      <Field id={nameField} label="Collaborator">
        <input
          id={nameField}
          required
          autoComplete="off"
          placeholder="login name"
          value={account}
          readOnly={disabledReason !== undefined}
          onChange={(event) => {
            setAccount(event.target.value);
          }}
          className={`w-40 ${FIELD} read-only:cursor-default read-only:opacity-50`}
        />
      </Field>
      <Field id={levelField} label="Reflection access">
        <select
          id={levelField}
          value={level}
          disabled={disabledReason !== undefined}
          onChange={(event) => {
            setLevel(event.target.value as GrantLevel);
          }}
          className={FIELD}
        >
          {GRANT_LEVELS.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      </Field>
      <button
        type="submit"
        aria-disabled={disabled}
        className={`${BUTTON.secondary} aria-disabled:opacity-50`}
      >
        Add collaborator
      </button>
    </form>
  );
}
